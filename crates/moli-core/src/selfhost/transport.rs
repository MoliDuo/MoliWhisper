//! One WebSocket per session to the self-hosted server.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use url::Url;

use super::protocol::{self, FINISH_FRAME};
use crate::asr::{AsrEvent, ConnectError, Handshake, SendError, ServerMsg};
use crate::net::{self, WsStream};

#[derive(Clone)]
pub struct ConnectOptions {
    pub url: Url,
    /// Sent as a bearer token when not empty.
    pub token: String,
    /// Budget for DNS + TCP + WebSocket handshake together.
    pub timeout: Duration,
}

impl ConnectOptions {
    /// `None` when `url` is not a `ws://` or `wss://` URL.
    pub fn new(url: &str, token: &str) -> Option<Self> {
        let url = Url::parse(url.trim()).ok()?;
        matches!(url.scheme(), "ws" | "wss").then(|| Self {
            url,
            token: token.trim().to_string(),
            timeout: Duration::from_secs(5),
        })
    }
}

pub async fn connect(
    opts: &ConnectOptions,
) -> Result<(SelfHostSink, SelfHostStream, Handshake), ConnectError> {
    let started = Instant::now();
    let ws = tokio::time::timeout(opts.timeout, handshake(opts))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    let (sink, stream) = ws.split();
    Ok((
        SelfHostSink { inner: sink },
        SelfHostStream {
            inner: stream,
            pending: VecDeque::new(),
            received_any: false,
            done: false,
        },
        Handshake {
            elapsed: started.elapsed(),
            set_cookies: Vec::new(),
        },
    ))
}

async fn handshake(opts: &ConnectOptions) -> Result<WsStream, ConnectError> {
    let mut request = opts
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| ConnectError::Transient(e.to_string()))?;
    if !opts.token.is_empty() {
        let value = HeaderValue::from_str(&format!("Bearer {}", opts.token))
            .map_err(|_| ConnectError::Transient("invalid token".into()))?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    match net::connect_websocket(request).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(response)) => {
            Err(ConnectError::Rejected(response.status().as_u16()))
        }
        Err(e) => Err(ConnectError::Transient(e.to_string())),
    }
}

/// Asks the server's `/healthz` (next to the WebSocket URL) and returns the
/// model it reports.
pub async fn health(url: &str, token: &str) -> Result<String, String> {
    let mut url = Url::parse(url.trim()).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return Err("the URL must start with ws:// or wss://".into()),
    };
    url.set_scheme(scheme).map_err(|_| "invalid URL")?;
    url.set_path("/healthz");
    url.set_query(None);
    let mut req = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5));
    if !token.trim().is_empty() {
        req = req.bearer_auth(token.trim());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("?")
        .to_string())
}

pub struct SelfHostSink {
    inner: SplitSink<WsStream, Message>,
}

impl SelfHostSink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        Ok(self.inner.send(Message::Binary(pcm.into())).await?)
    }

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

pub struct SelfHostStream {
    inner: SplitStream<WsStream>,
    /// Messages of one frame still to hand out (`final` makes two).
    pending: VecDeque<ServerMsg>,
    received_any: bool,
    done: bool,
}

impl SelfHostStream {
    /// Next event; `None` after `Closed` or `Failed` has been returned.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        if let Some(msg) = self.pending.pop_front() {
            return Some(AsrEvent::Server(msg));
        }
        if self.done {
            return None;
        }
        loop {
            let event = match self.inner.next().await {
                Some(Ok(Message::Text(text))) => {
                    self.received_any = true;
                    match protocol::parse(&text) {
                        Ok(frame) => {
                            let mut msgs = frame.into_msgs().into_iter();
                            let first = msgs.next().expect("a frame gives at least one message");
                            self.pending.extend(msgs);
                            AsrEvent::Server(first)
                        }
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
                Some(Ok(_)) => continue,
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
