//! One WebSocket per session: connect with the login cookies, stream PCM up,
//! read JSON events down.

use std::time::{Duration, Instant};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, SET_COOKIE, USER_AGENT};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use url::Url;

use super::credentials::Credentials;
use super::params::{self, Overrides};
use super::protocol::{self, FINISH_FRAME};
use crate::asr::{AsrEvent, ConnectError, Handshake, SendError};
use crate::doubao::net::{self, WsStream};

/// The browser the service is told it talks to.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Everything needed to open a session.
#[derive(Clone)]
pub struct ConnectOptions {
    pub url: Url,
    pub cookie_header: String,
    pub origin: Option<String>,
    pub user_agent: Option<String>,
    /// Budget for DNS + TCP + TLS + WebSocket handshake together. Some edges
    /// stall TLS forever, so keep this short and retry instead.
    pub timeout: Duration,
}

impl ConnectOptions {
    /// Options for a session under `creds`, as a fresh browser tab would open it.
    pub fn new(creds: &Credentials, overrides: &Overrides) -> Self {
        Self {
            url: params::url(&creds.identity(), &params::new_web_tab_id(), overrides),
            cookie_header: creds.cookie_header(),
            origin: Some(params::ORIGIN.to_string()),
            user_agent: Some(DEFAULT_USER_AGENT.to_string()),
            timeout: Duration::from_secs(5),
        }
    }
}

/// Opens a session.
pub async fn connect(
    opts: &ConnectOptions,
) -> Result<(WebSink, WebStream, Handshake), ConnectError> {
    let started = Instant::now();
    let (ws, set_cookies) = tokio::time::timeout(opts.timeout, handshake(opts))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    let (sink, stream) = ws.split();
    Ok((
        WebSink { inner: sink },
        WebStream {
            inner: stream,
            received_any: false,
            done: false,
        },
        Handshake {
            elapsed: started.elapsed(),
            set_cookies,
        },
    ))
}

async fn handshake(opts: &ConnectOptions) -> Result<(WsStream, Vec<String>), ConnectError> {
    let mut request = opts
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| ConnectError::Transient(e.to_string()))?;
    let headers = request.headers_mut();
    let header = |v: &str| {
        HeaderValue::from_str(v).map_err(|_| ConnectError::Transient("invalid header value".into()))
    };
    headers.insert(COOKIE, header(&opts.cookie_header)?);
    if let Some(origin) = &opts.origin {
        headers.insert(ORIGIN, header(origin)?);
    }
    if let Some(ua) = &opts.user_agent {
        headers.insert(USER_AGENT, header(ua)?);
    }

    match net::connect_websocket(request).await {
        Ok((ws, response)) => {
            let set_cookies = response
                .headers()
                .get_all(SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_owned))
                .collect();
            Ok((ws, set_cookies))
        }
        Err(tungstenite::Error::Http(response)) => {
            Err(ConnectError::Rejected(response.status().as_u16()))
        }
        Err(e) => Err(ConnectError::Transient(e.to_string())),
    }
}

/// The sending half of a session.
pub struct WebSink {
    inner: SplitSink<WsStream, Message>,
}

impl WebSink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        Ok(self.inner.send(Message::Binary(pcm.into())).await?)
    }

    /// Tells the service the audio is over; it answers with the final result and `finish`.
    pub async fn finish(&mut self) -> Result<(), SendError> {
        Ok(self.inner.send(Message::Text(FINISH_FRAME.into())).await?)
    }

    pub async fn close(&mut self) -> Result<(), SendError> {
        let frame = CloseFrame {
            code: CloseCode::Normal,
            reason: "".into(),
        };
        Ok(self.inner.send(Message::Close(Some(frame))).await?)
    }
}

/// The receiving half of a session.
pub struct WebStream {
    inner: SplitStream<WsStream>,
    received_any: bool,
    done: bool,
}

impl WebStream {
    /// Next event; `None` after `Closed` or `Failed` has been returned.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        if self.done {
            return None;
        }
        loop {
            let event = match self.inner.next().await {
                Some(Ok(Message::Text(text))) => {
                    self.received_any = true;
                    match protocol::parse(&text) {
                        Ok(msg) => AsrEvent::Server(msg),
                        Err(_) => AsrEvent::Garbage(text.chars().take(200).collect()),
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    self.done = true;
                    let (code, reason) = frame
                        .map(|f| (Some(u16::from(f.code)), f.reason.to_string()))
                        .unwrap_or((None, String::new()));
                    AsrEvent::Closed {
                        code,
                        reason,
                        received_any: self.received_any,
                    }
                }
                Some(Ok(_)) => continue, // binary, ping, pong
                Some(Err(tungstenite::Error::ConnectionClosed)) | None => {
                    self.done = true;
                    AsrEvent::Closed {
                        code: None,
                        reason: String::new(),
                        received_any: self.received_any,
                    }
                }
                Some(Err(e)) => {
                    self.done = true;
                    AsrEvent::Failed(e.to_string())
                }
            };
            return Some(event);
        }
    }
}
