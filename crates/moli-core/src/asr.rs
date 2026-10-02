//! Speech recognition as the session sees it.
//!
//! A session connects with [`crate::qwen::connect`] and gets a sink for audio
//! and a stream of [`AsrEvent`]s; the types here are what the two sides share.

use std::time::Duration;

use tokio_tungstenite::tungstenite;

/// Audio goes up as 16 kHz mono s16le PCM.
pub const SAMPLE_RATE: u32 = 16_000;

/// What the service tells the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    /// The full transcript so far (not a delta).
    Result {
        text: String,
    },
    /// The service is done; no more results follow.
    Finish,
    Error {
        code: i64,
        message: String,
    },
    Unknown {
        event: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEvent {
    Server(ServerMsg),
    /// A text frame that was not valid JSON (truncated).
    Garbage(String),
    /// The connection ended.
    Closed {
        code: Option<u16>,
        reason: String,
    },
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The service refused the API key.
    #[error("the API key was rejected")]
    KeyRejected,
    /// The upgrade was answered with another HTTP status.
    #[error("handshake rejected with HTTP {0}")]
    Rejected(u16),
    #[error("connect timed out")]
    Timeout,
    /// Network trouble or an unexpected server response; worth retrying.
    #[error("{0}")]
    Transient(String),
}

#[derive(Debug)]
pub struct Handshake {
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("send failed: {0}")]
    Ws(#[from] tungstenite::Error),
    #[error("send failed: the session is over")]
    Closed,
}
