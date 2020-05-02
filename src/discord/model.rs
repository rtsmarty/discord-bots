use bytes::Bytes;
use serde_derive::{Serialize, Deserialize};
use std::borrow::Cow;

pub fn bytes_from_cow(parent: &Bytes, cow: Cow<str>) -> Bytes {
    match cow {
        Cow::Owned(s)    => Bytes::from(s),
        Cow::Borrowed(s) => parent.slice_ref(s.as_bytes()),
    }
}

#[derive(Serialize, Deserialize)]
pub struct WsPayload<T> {
    pub op: i32,
    pub d: T,
    #[serde(skip_serializing_if="Option::is_none")]
    pub s: Option<u64>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub t: Option<String>
}
#[derive(Deserialize)]
pub struct WsPayloadUnknownOp {
    pub op: i32,
    #[serde(skip_serializing_if="Option::is_none")]
    pub s: Option<u64>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub t: Option<String>
}
#[derive(Deserialize)]
pub struct Hello {
    pub heartbeat_interval: u64,
}
#[derive(Serialize)]
pub struct Identify<'a> {
    pub token: &'a str,
    pub properties: IdentifyProperties<'a>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub compress: Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub large_threshold: Option<u16>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub shard: Option<[i32; 2]>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub presence: Option<UpdateStatus<'a>>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub guild_subscriptions: Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub intents: Option<i32>
}
#[derive(Serialize)]
pub struct IdentifyProperties<'a> {
    #[serde(rename="$os")]
    pub os: &'a str,
    #[serde(rename="$browser")]
    pub browser: &'a str,
    #[serde(rename="$device")]
    pub device: &'a str,
}
#[derive(Serialize)]
pub struct UpdateStatus<'a> {
    #[serde(skip_serializing_if="Option::is_none")]
    pub since: Option<u64>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub game: Option<Activity<'a>>,
    pub status: &'a str,
    pub afk: bool
}
#[derive(Deserialize, Serialize)]
pub struct Activity<'a> {
    pub name: &'a str,
    #[serde(rename="type")]
    pub ty: i32,
    #[serde(skip_serializing_if="Option::is_none")]
    pub url: Option<&'a str>,
}
#[derive(Deserialize)]
pub struct Ready<'a> {
    pub session_id: Cow<'a, str>,
    pub user: User<'a>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // shard: Option<[u32; 2]>,
}
#[derive(Deserialize)]
pub struct User<'a> {
    pub id: Cow<'a, str>,
    // username: Cow<'a, str>,
    // discriminator: Cow<'a, str>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // avatar: Option<Cow<'a, str>>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // bot: Option<bool>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // mfa_enabled: Option<bool>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // locale: Option<Cow<'a, str>>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // verified: Option<bool>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // email: Option<Cow<'a, str>>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // flags: Option<i32>,
    // #[serde(skip_serializing_if="Option::is_none")]
    // premium_type: Option<i32>,
}

#[derive(Serialize)]
pub struct Resume<'a> {
    pub token: Cow<'a, str>,
    pub session_id: Cow<'a, str>,
    pub seq: u64,
}

#[derive(Deserialize)]
pub struct MessageReceived<'a> {
    pub id: Cow<'a, str>,
    pub channel_id: Cow<'a, str>,
    pub guild_id: Option<Cow<'a, str>>,
    pub content: Cow<'a, str>,
    pub mentions: Vec<User<'a>>,
    pub author: User<'a>,
}

#[derive(Debug, Deserialize)]
pub struct BotGatewaySessionStartLimit {
    pub total: u64,
    pub remaining: u64,
    pub reset_after: u64
}
#[derive(Debug, Deserialize)]
pub struct BotGatewayResponse<'a> {
    pub url: &'a str,
    pub shards: i32,
    pub session_start_limit: BotGatewaySessionStartLimit
}
#[derive(Debug, Serialize)]
pub struct CreateMessageRequest<'a> {
    pub content: &'a str,
}