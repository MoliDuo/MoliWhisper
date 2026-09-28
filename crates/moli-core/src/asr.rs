//! Speech recognition as the session sees it, whichever service does it.
//!
//! A session connects a [`Backend`] and gets a [`Sink`] for audio and a
//! [`Stream`] of [`AsrEvent`]s. Both Doubao services, the web ASR (needs a
//! login) and the IME (anonymous), look the same through here; see
//! [`crate::doubao`] for the clients themselves.

use std::time::Duration;

use tokio_tungstenite::tungstenite;

use crate::doubao::ime::{ImeClient, ImeSink, ImeStream};
use crate::doubao::web::{self, ConnectOptions, WebSink, WebStream};

/// Audio goes up as 16 kHz mono s16le PCM.
pub const SAMPLE_RATE: u32 = 16_000;

/// The recognition service for a session.
#[derive(Clone)]
pub enum Backend {
    Web(ConnectOptions),
    Ime(ImeClient),
}

impl Backend {
    /// Connects within `timeout` (DNS, TLS, WebSocket and, for the IME,
    /// credentials and the session handshake).
    pub async fn connect(
        &self,
        timeout: Duration,
    ) -> Result<(Sink, Stream, Handshake), ConnectError> {
        match self {
            Backend::Web(opts) => {
                let opts = ConnectOptions {
                    timeout,
                    ..opts.clone()
                };
                let (sink, stream, handshake) = web::connect(&opts).await?;
                Ok((Sink::Web(sink), Stream::Web(stream), handshake))
            }
            Backend::Ime(ime) => {
                let (sink, stream, handshake) = ime.connect(timeout).await?;
                Ok((Sink::Ime(sink), Stream::Ime(stream), handshake))
            }
        }
    }
}

/// Where the audio goes.
pub enum Sink {
    Web(WebSink),
    Ime(ImeSink),
}

impl Sink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.audio(pcm).await,
            Sink::Ime(s) => s.audio(pcm),
        }
    }

    /// Tells the service the audio is over; it answers with the final result and `Finish`.
    pub async fn finish(&mut self) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.finish().await,
            Sink::Ime(s) => s.finish(),
        }
    }

    pub async fn close(&mut self) -> Result<(), SendError> {
        match self {
            Sink::Web(s) => s.close().await,
            Sink::Ime(s) => s.close(),
        }
    }
}

/// Where the results come from.
pub enum Stream {
    Web(WebStream),
    Ime(ImeStream),
}

impl Stream {
    /// Next event; `None` once the session is over.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        match self {
            Stream::Web(s) => s.next().await,
            Stream::Ime(s) => s.next().await,
        }
    }
}

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
    /// The connection ended. `received_any` tells whether any server message
    /// arrived first. A clean close with none is ambiguous: reason `"2013"`
    /// means the session was refused, `"1000-"` that no audio came in time.
    Closed {
        code: Option<u16>,
        reason: String,
        received_any: bool,
    },
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The upgrade was answered with a plain HTTP status. From the web ASR,
    /// 403 means a missing `Origin` or a WAF block; with no cookies at all it
    /// answers 200. An invalid login does not show up here: the handshake
    /// succeeds and the service rejects the session afterwards (see
    /// [`web::protocol::is_session_rejected`]).
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
    /// Raw `Set-Cookie` headers from the upgrade response. Secret.
    pub set_cookies: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("send failed: {0}")]
    Ws(#[from] tungstenite::Error),
    #[error("send failed: the session is over")]
    Closed,
}
