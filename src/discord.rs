use bitflags::bitflags;
use bytes::{
    Bytes,
    BytesMut,
};
use crate::{
    error::Error,
    ws,
};
use futures::{
    future::FutureExt,
    pin_mut,
    stream::StreamExt,
};
use hyper::{
    client::{
        Client,
        HttpConnector,
    },
    upgrade::Upgraded,
    Body,
    Request,
    Response,
};
use crate::{
    tls::{
        HttpsConnector,
        TlsStream,
    },
};
use tokio::{
    io::{
        split,
        AsyncRead,
        AsyncWrite,
        ReadHalf,
        WriteHalf
    },
    net::TcpStream,
    time::{
        delay_for,
        Delay,
        interval,
        Interval,
    },
};
use std::{
    borrow::Cow,
    cmp,
    future::Future,
    marker::Unpin,
    str::{
        self,
        FromStr,
    },
    time::Duration,
};
use unicase::UniCase;

mod model;

type HttpsClient = Client<HttpsConnector<HttpConnector>>;

#[derive(Debug)]
pub struct Message {
    channel_id: Bytes,
    guild_id: Option<Bytes>,
    content: Bytes,
    author_id: Bytes,
    message_id: Bytes,
    mentioned: bool,
    is_me: bool,
}
impl Message {
    fn from_message_received(bytes: &Bytes, msg: model::MessageReceived, uid: &[u8]) -> Self {
        Self {
            is_me: msg.author.id.as_bytes() == uid,
            mentioned: msg.mentions.iter().any(|u| u.id.as_bytes() == uid),

            message_id: model::bytes_from_cow(&bytes, msg.id),
            channel_id: model::bytes_from_cow(&bytes, msg.channel_id),
            guild_id: msg.guild_id.map(|c| model::bytes_from_cow(&bytes, c)),
            author_id: model::bytes_from_cow(&bytes, msg.author.id),
            content: model::bytes_from_cow(&bytes, msg.content),
        }
    }
    pub fn channel_id(&self) -> &str {
        unsafe { str::from_utf8_unchecked(&*self.channel_id) }
    }
    pub fn channel_id_buf(&self) -> &Bytes {
        &self.channel_id
    }
    pub fn guild_id(&self) -> Option<&str> {
        unsafe { self.guild_id.as_ref().map(|b| str::from_utf8_unchecked(&b)) }
    }
    pub fn guild_id_buf(&self) -> Option<&Bytes> {
        self.guild_id.as_ref()
    }
    pub fn message_id(&self) -> &str {
        unsafe { str::from_utf8_unchecked(&*self.message_id) }
    }
    pub fn message_id_buf(&self) -> &Bytes {
        &self.message_id
    }
    pub fn message(&self) -> &str {
        unsafe { str::from_utf8_unchecked(&*self.content) }
    }
    pub fn message_buf(&self) -> &Bytes {
        &self.content
    }
    pub fn author_id(&self) -> &str {
        unsafe { str::from_utf8_unchecked(&*self.author_id) }
    }
    pub fn author_id_buf(&self) -> &Bytes {
        &self.author_id
    }
    pub fn mentioned(&self) -> bool {
        self.mentioned
    }
    pub fn is_me(&self) -> bool {
        self.is_me
    }
}

pub struct ChannelMessages {
    client:       HttpsClient,
    auth_header:  http::HeaderValue,
    user_id:      Bytes,
    base_uri:     String,
    next_res:     Option<std::vec::IntoIter<Message>>,
    next_msg_id:  Option<String>,
    limit:        Option<usize>,
    rate_limiter: Option<Delay>,
}
impl ChannelMessages {
    pub async fn next(&mut self) -> Result<Option<Message>, Error> {
        loop {
            match self.next_res.take() {
                Some(mut vec) => {
                    let next = vec.next();
                    if let Some(next) = next {
                        self.next_res = Some(vec);
                        self.next_msg_id = Some(next.message_id().to_string());
                        return Ok(Some(next));
                    } else {
                        self.next_res = None;
                    }
                }
                None => {
                    let limit = match self.limit {
                        Some(0) => return Ok(None),
                        Some(ref mut n) => {
                            let next = cmp::min(*n, 100);
                            *n = n.saturating_sub(100);
                            next
                        }
                        None => 100
                    };

                    if let Some(delay) = self.rate_limiter.take() {
                        delay.await;
                    }
                    let uri = match self.next_msg_id.take() {
                        Some(msg_id) => format!("{}?limit={}&before={}", self.base_uri, limit, msg_id),
                        None => format!("{}?limit={}", self.base_uri, limit),
                    };

                    let req = Request::get(uri)
                        .header(http::header::AUTHORIZATION, self.auth_header.clone())
                        .body(Body::empty())?;

                    let bytes = Discord::get_success_response_bytes(&self.client, req).await?;
                    self.rate_limiter = Some(delay_for(Duration::from_secs(10)));

                    let response = serde_json::from_slice::<Vec<model::MessageReceived>>(&bytes)?;
                    let next_res = response.into_iter()
                        .map(|msg| Message::from_message_received(&bytes, msg, &self.user_id))
                        .collect::<Vec<_>>();
                    if next_res.len() < limit {
                        self.limit = Some(0);
                    }
                    self.next_res = Some(next_res.into_iter());
                }
            }
        }
    }
}

bitflags! {
    pub struct Intents: i32 {
        const GUILDS                   = 1 << 0;
        const GUILD_MEMBERS            = 1 << 1;
        const GUILD_BANS               = 1 << 2;
        const GUILD_EMOJIS             = 1 << 3;
        const GUILD_INTEGRATIONS       = 1 << 4;
        const GUILD_WEBHOOKS           = 1 << 5;
        const GUILD_INVITES            = 1 << 6;
        const GUILD_VOICE_STATES       = 1 << 7;
        const GUILD_PRESENCES          = 1 << 8;
        const GUILD_MESSAGES           = 1 << 9;
        const GUILD_MESSAGE_REACTIONS  = 1 << 10;
        const GUILD_MESSAGE_TYPING     = 1 << 11;
        const DIRECT_MESSAGES          = 1 << 12;
        const DIRECT_MESSAGE_REACTIONS = 1 << 13;
        const DIRECT_MESSAGE_TYPING    = 1 << 14;
    }
}


#[derive(Debug)]
pub struct Discord {
    client: HttpsClient,
    prebuf: Option<Bytes>,
    wsreader: ReadHalf<TlsStream<TcpStream>>,
    wswriter: WriteHalf<TlsStream<TcpStream>>,
    token: String,
    auth_header: http::HeaderValue,
    session_id: Bytes,
    last_seq: u64,
    heartbeat_interval: Interval,
    user_id: Bytes,
    ack: Option<()>,
}
impl Discord {
    const GATEWAY_PARAMETERS: &'static str = "?v=6&encoding=json";
    const BOT_AUTH_HEADER_PREFIX: &'static str = "Bot ";

    pub async fn connect_bot(token: &str, intents: Option<Intents>) -> Result<Discord, Error> {
        let client = Client::builder().build(HttpsConnector::new()?);

        let mut bot_auth_buf = BytesMut::with_capacity(Self::BOT_AUTH_HEADER_PREFIX.len() + token.len());
        bot_auth_buf.extend_from_slice(Self::BOT_AUTH_HEADER_PREFIX.as_bytes());
        bot_auth_buf.extend_from_slice(token.as_bytes());
        let auth_header_bytes = bot_auth_buf.freeze();

        let auth_header = http::HeaderValue::from_maybe_shared(auth_header_bytes).map_err(|e| Error::Http(e.into()))?;

        let gateway_url_bytes = Self::bot_gateway_url(&client, auth_header.clone()).await?;
        let mut urlbuf = BytesMut::from(&*gateway_url_bytes);
        urlbuf.reserve(Self::GATEWAY_PARAMETERS.len());
        urlbuf.extend_from_slice(Self::GATEWAY_PARAMETERS.as_bytes());

        let upgrade = Self::connect_gateway(&client, auth_header.clone(), urlbuf.freeze()).await?;
        let stream = upgrade.downcast::<TlsStream<TcpStream>>().unwrap();
        let prebuf = if stream.read_buf.len() > 0 { Some(stream.read_buf) } else { None };
        let mut wsstream = stream.io;

        let owned_message = ws::message::Owned::read(&mut wsstream).await?;
        let hello = match owned_message.message() {
            ws::Message::Text(t) => serde_json::from_str::<model::WsPayload<model::Hello>>(t)?,
            _ => panic!()
        };

        let heartbeat_interval = interval(Duration::from_millis(hello.d.heartbeat_interval));

        let ready_message = Self::identify_handshake(&mut wsstream, token, intents).await?;
        let ready = match ready_message.message() {
            ws::Message::Text(t) => serde_json::from_str::<model::WsPayload<model::Ready>>(t)?,
            _ => panic!()
        };

        let last_seq = ready.s.unwrap_or(0);
        let session_id = model::bytes_from_cow(ready_message.buf(), ready.d.session_id);
        let user_id = model::bytes_from_cow(ready_message.buf(), ready.d.user.id);

        let (wsreader, wswriter) = split(wsstream);

        Ok(Discord {
            client,
            prebuf,
            wsreader,
            wswriter,
            token: String::from(token),
            auth_header,
            session_id,
            last_seq,
            heartbeat_interval,
            user_id,
            ack: Some(()),
        })
    }

    pub async fn reconnect(&mut self) -> Result<(), Error> {
        let gateway_url_bytes = Self::bot_gateway_url(&self.client, self.auth_header.clone()).await?;
        let mut urlbuf = BytesMut::from(&*gateway_url_bytes);
        urlbuf.reserve(Self::GATEWAY_PARAMETERS.len());
        urlbuf.extend_from_slice(Self::GATEWAY_PARAMETERS.as_bytes());

        let upgrade = Self::connect_gateway(&self.client, self.auth_header.clone(), urlbuf.freeze()).await?;
        let stream = upgrade.downcast::<TlsStream<TcpStream>>().unwrap();
        let prebuf = if stream.read_buf.len() > 0 { Some(stream.read_buf) } else { None };
        let mut wsstream = stream.io;

        let owned_message = ws::message::Owned::read(&mut wsstream).await?;
        let hello = match owned_message.message() {
            ws::Message::Text(t) => serde_json::from_str::<model::WsPayload<model::Hello>>(t)?,
            _ => panic!()
        };

        self.heartbeat_interval = interval(Duration::from_millis(hello.d.heartbeat_interval));

        ws::Message::Text(&serde_json::to_string(&model::WsPayload {
                op: 6,
                d: model::Resume {
                    token: Cow::Borrowed(&self.token),
                    session_id: Cow::Borrowed(self.session_id()),
                    seq: self.last_seq,
                },
                s: None,
                t: None
            })?)
            .write(&mut wsstream, ws::message::Context::Client).await?;

        let (wsreader, wswriter) = split(wsstream);

        self.wsreader = wsreader;
        self.wswriter = wswriter;
        self.prebuf   = prebuf;

        Ok(())
    }

    pub fn user_id(&self) -> &str {
        // safety: self.user_id always comes from a Cow<str> so will always be
        // UTF-8
        unsafe { str::from_utf8_unchecked(&*self.user_id) }
    }
    pub fn session_id(&self) -> &str {
        // safety: self.session_id always comes from a Cow<str> so will always
        // be UTF-8
        unsafe { str::from_utf8_unchecked(&*self.session_id) }
    }

    async fn get_success_response(client: &HttpsClient, req: Request<Body>) -> Result<Response<Body>, Error> {
        let res = client.request(req).await?;
        let status = res.status();
        if !status.is_success() {
            let length = res.headers()
                .get(http::header::CONTENT_LENGTH)
                .and_then(|hv| str::from_utf8(hv.as_bytes()).ok())
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let mut res_body = res.into_body();

            let mut buffer = BytesMut::with_capacity(length);
            while let Some(chunk) = res_body.next().await {
                let chunk = chunk?;
                buffer.reserve(chunk.len());
                buffer.extend_from_slice(&chunk);
            }
            Err(Error::BadApiRequest(buffer.freeze()))
        } else {
            Ok(res)
        }
    }
    async fn get_success_response_bytes(client: &HttpsClient, req: Request<Body>) -> Result<Bytes, Error> {
        let res = client.request(req).await?;
        let status = res.status();
        let length = res.headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|hv| str::from_utf8(hv.as_bytes()).ok())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let mut res_body = res.into_body();

        let mut buffer = BytesMut::with_capacity(length);
        while let Some(chunk) = res_body.next().await {
            let chunk = chunk?;
            buffer.reserve(chunk.len());
            buffer.extend_from_slice(&chunk);
        }
        let bytes = buffer.freeze();

        if !status.is_success() {
            Err(Error::BadApiRequest(bytes))
        } else {
            Ok(bytes)
        }
    }

    pub async fn next(&mut self) -> Result<Message, Error> {
        let user_id = self.user_id.clone();

        // loop until we get a message that's a proper discord message that we
        // care about (i.e. not a Heartbeat Ack/Reaction/etc, actually a text
        // message sent to a channel)
        loop {
            let reconnect = {
                let message = ws::message::Owned::read(&mut self.wsreader).fuse();
                pin_mut!(message);

                // We also need to send a heartbeat occassionally, so loop until we
                // get something that isn't our heartbeat interval (i.e. actually
                // a proper websocket message)
                let (msg, reconnect) = loop {
                    let interval = self.heartbeat_interval.tick().fuse();
                    pin_mut!(interval);

                    // Prefer sending heartbeats over receiving messages if we can
                    futures::select_biased! {
                        _ = interval => match self.ack.take() {
                            Some(()) => {
                                let identify = model::WsPayload {
                                    op: 1,
                                    d: self.last_seq,
                                    s: None,
                                    t: None,
                                };
                                let serialized = serde_json::to_string(&identify)?;
                                ws::Message::Text(&serialized)
                                    .write(&mut self.wswriter, ws::message::Context::Client)
                                    .await?;
                            }
                            None => return Err(Error::NoAck),
                        },
                        msg_res = message => break {
                            let owned_message = msg_res?;

                            match owned_message.message() {
                                ws::Message::Text(t) => {
                                    let next = serde_json::from_str::<model::WsPayloadUnknownOp>(t)?;

                                    if let Some(s) = next.s {
                                        self.last_seq = s;
                                    }

                                    if next.op == 11 {
                                        self.ack = Some(());
                                    }
                                    if let Some("MESSAGE_CREATE") = next.t.as_deref() {
                                        let msg = serde_json::from_str::<model::WsPayload<model::MessageReceived>>(t)?;
                                        (Some(Message::from_message_received(owned_message.buf(), msg.d, &user_id)), false)
                                    } else {
                                        (None, false)
                                    }
                                }
                                ws::Message::Close(Some((1001, _))) => {
                                    (None, true)
                                }
                                _ => return Err(Error::UnexpectedWebsocketResponse(owned_message))
                            }
                        }
                    };
                };

                if let Some(msg) = msg {
                    break Ok(msg);
                }
                reconnect
            };
            if reconnect {
                self.reconnect().await?;
            }
        }
    }

    pub fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> impl Future<Output=Result<(), Error>> + Send + 'static {
        let uri = format!("https://discordapp.com/api/v6/channels/{}/messages/{}/reactions/{}/@me",
                          channel_id, message_id, emoji);
        let req = Request::put(uri)
            .header(http::header::AUTHORIZATION, self.auth_header.clone())
            .header(http::header::CONTENT_LENGTH, 0)
            .body(Body::empty());

        let client = self.client.clone();
        async move {
            Self::get_success_response(&client, req?).await.map(|_| ())
        }
    }
    pub fn send_message(&self, channel_id: &str, message: &str) -> impl Future<Output=Result<(), Error>> + Send + 'static {
        let uri = format!("https://discordapp.com/api/v6/channels/{}/messages", channel_id);
        let req: Result<Request<Body>, Error> = try {
            Request::post(uri)
                .header(http::header::AUTHORIZATION, self.auth_header.clone())
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&model::CreateMessageRequest { content: message })?))?
        };
        let client = self.client.clone();
        async move {
            Self::get_success_response(&client, req?).await.map(|_| ())
        }
    }
    pub fn channel_messages(&self, channel_id: &str, limit: Option<usize>, before_msg: Option<String>) -> ChannelMessages {
        ChannelMessages {
            auth_header: self.auth_header.clone(),
            base_uri: format!("https://discordapp.com/api/v6/channels/{}/messages", channel_id),
            client: self.client.clone(),
            limit,
            next_msg_id: before_msg,
            next_res: None,
            rate_limiter: None,
            user_id: self.user_id.clone(),
        }
    }
    async fn bot_gateway_url(client: &HttpsClient, auth_header: http::HeaderValue) -> Result<Bytes, Error> {
        let req = Request::get("https://discordapp.com/api/v6/gateway/bot")
            .header(http::header::AUTHORIZATION, auth_header)
            .body(Body::empty())?;

        let bytes = Self::get_success_response_bytes(client, req).await?;
        let response = serde_json::from_slice::<model::BotGatewayResponse>(&bytes)?;
        Ok(bytes.slice_ref(response.url.as_bytes()))
    }
    async fn connect_gateway(client: &HttpsClient, auth_header: http::HeaderValue, gateway_url: Bytes) -> Result<Upgraded, Error> {
        let nonce = ws::RequestKey::generate()?;
        let req = Request::get(&*gateway_url)
            .header(http::header::AUTHORIZATION, auth_header)
            .header(http::header::UPGRADE, "websocket")
            .header(http::header::CONNECTION, "upgrade")
            .header(http::header::SEC_WEBSOCKET_VERSION, "13")
            .header(http::header::SEC_WEBSOCKET_KEY, nonce.as_ref())
            .body(Body::empty())?;

        let res = Self::verify_ws_handshake_response(&nonce, client.request(req).await?)?;
        Ok(res.into_body().on_upgrade().await?)
    }
    fn verify_ws_handshake_response(nonce: &ws::RequestKey, res: Response<Body>) -> Result<Response<Body>, Error> {
        if res.status() != http::status::StatusCode::SWITCHING_PROTOCOLS {
            return Err(Error::Handshake(res));
        }
        if res.headers()
            .get(http::header::UPGRADE)
            .and_then(|h| h.to_str().ok())
            .map(UniCase::new) != Some(UniCase::new("WEBSOCKET"))
        {
            return Err(Error::Handshake(res));
        }
        if res.headers()
            .get(http::header::CONNECTION)
            .and_then(|h| h.to_str().ok())
            .map(UniCase::new) != Some(UniCase::new("UPGRADE"))
        {
            return Err(Error::Handshake(res));
        }
        if let Some(value) = res.headers()
            .get(http::header::SEC_WEBSOCKET_ACCEPT)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| ws::ResponseKey::from_str(h).ok())
        {
            if !nonce.verify(value) {
                return Err(Error::Handshake(res));
            }
        } else {
            return Err(Error::Handshake(res));
        }

        Ok(res)
    }

    async fn identify_handshake<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, token: &str, intents: Option<Intents>) -> Result<ws::message::Owned, Error> {
        ws::Message::Text(&serde_json::to_string(&model::WsPayload {
                op: 2,
                d: model::Identify {
                    token: token,
                    properties: model::IdentifyProperties {
                        os: "linux",
                        browser: "tokio",
                        device: "server",
                    },
                    compress: Some(false),
                    large_threshold: None,
                    shard: None,
                    presence: None,
                    guild_subscriptions: Some(false),
                    intents: intents.map(|i| i.bits())
                },
                s: None,
                t: None
            })?)
            .write(stream, ws::message::Context::Client).await?;

        ws::message::Owned::read(stream).await.map_err(Error::from)
    }
}
