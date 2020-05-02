#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Connection failure")]
    Hyper(#[from] hyper::Error),
    #[error("Connection TLS failure")]
    Tls(#[from] native_tls::Error),
    #[error("Http failure")]
    Http(#[from] http::Error),
    #[error("Tokio I/O failure")]
    TokioIo(#[from] tokio::io::Error),
    #[error("De/Serialization failure")]
    Serde(#[from] serde_json::Error),
    #[error("Randomness failure")]
    Rand(#[from] rand::Error),
    #[error("Invalid Websocket Handshake Response")]
    Handshake(hyper::Response<hyper::Body>),
    #[error("Websocket Error")]
    WebSocket(#[from] crate::ws::message::Error),
    #[error("An Unknown Error happened")]
    UnknownError(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("API request responsed with non-success status, body: {0:?}")]
    BadApiRequest(bytes::Bytes),
    #[error("Unexpected Websocket response: {0:?}")]
    UnexpectedWebsocketResponse(crate::ws::message::Owned),
    #[error("No ack received between heartbeats")]
    NoAck,
    #[error("A channel was closed when it shouldn't have been")]
    SendChannelClosed,
}
