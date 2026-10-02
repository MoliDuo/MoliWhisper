//! One WebSocket per session: connect with the API key, open the task, stream
//! PCM up and read events down.

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
use uuid::Uuid;

use super::protocol::{self, Frame};
use super::{DEFAULT_MODEL, DEFAULT_URL};
use crate::asr::{AsrEvent, ConnectError, Handshake, SAMPLE_RATE, SendError, ServerMsg};
use crate::net::{self, WsStream};

#[derive(Clone)]
pub struct ConnectOptions {
    pub url: Url,
    pub api_key: String,
    pub model: String,
    /// Budget for DNS + TLS + WebSocket handshake + `task-started` together.
    pub timeout: Duration,
}

impl ConnectOptions {
    /// `None` when there is no key or the URL is not a WebSocket URL. A blank
    /// URL or model means the default.
    pub fn new(url: &str, api_key: &str, model: &str) -> Option<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return None;
        }
        let url = Url::parse(non_blank(url, DEFAULT_URL)).ok()?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return None;
        }
        Some(Self {
            url,
            api_key: api_key.to_string(),
            model: non_blank(model, DEFAULT_MODEL).to_string(),
            timeout: Duration::from_secs(5),
        })
    }
}

fn non_blank<'a>(value: &'a str, default: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { default } else { value }
}

/// Opens a session: connects, starts the task and waits until the service is
/// ready for audio.
pub async fn connect(
    opts: &ConnectOptions,
) -> Result<(QwenSink, QwenStream, Handshake), ConnectError> {
    let started = Instant::now();
    let (sink, stream) = tokio::time::timeout(opts.timeout, open(opts))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    Ok((
        sink,
        stream,
        Handshake {
            elapsed: started.elapsed(),
            set_cookies: Vec::new(),
        },
    ))
}

async fn open(opts: &ConnectOptions) -> Result<(QwenSink, QwenStream), ConnectError> {
    let mut request = opts
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| ConnectError::Transient(e.to_string()))?;
    let bearer = HeaderValue::from_str(&format!("Bearer {}", opts.api_key))
        .map_err(|_| ConnectError::Transient("the API key has invalid characters".into()))?;
    request.headers_mut().insert(AUTHORIZATION, bearer);

    let ws = match net::connect_websocket(request).await {
        Ok((ws, _)) => ws,
        Err(tungstenite::Error::Http(response)) => {
            return Err(ConnectError::Rejected(response.status().as_u16()));
        }
        Err(e) => return Err(ConnectError::Transient(e.to_string())),
    };
    let (mut sink, mut stream) = ws.split();

    let task_id = Uuid::new_v4().simple().to_string();
    let run = protocol::run_task(&task_id, &opts.model, SAMPLE_RATE);
    sink.send(Message::Text(run.into()))
        .await
        .map_err(|e| ConnectError::Transient(e.to_string()))?;

    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => match protocol::parse(&text) {
                Ok(Frame::Started) => break,
                Ok(Frame::Failed { code, message }) => {
                    return Err(match code.as_str() {
                        "InvalidApiKey" | "AccessDenied" => ConnectError::Rejected(401),
                        _ => ConnectError::Transient(format!("{code}: {message}")),
                    });
                }
                _ => {}
            },
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(ConnectError::Transient(e.to_string())),
            None => {
                return Err(ConnectError::Transient(
                    "closed before the task started".into(),
                ));
            }
        }
    }

    Ok((
        QwenSink {
            inner: sink,
            task_id,
        },
        QwenStream {
            inner: stream,
            finished_text: String::new(),
            received_any: true,
            done: false,
        },
    ))
}

/// Checks the key, the model and the URL by starting and ending a task.
pub async fn check(opts: &ConnectOptions) -> Result<Duration, String> {
    let (mut sink, _stream, handshake) = connect(opts).await.map_err(|e| match e {
        ConnectError::Rejected(401 | 403) => "the API key was rejected".to_string(),
        other => other.to_string(),
    })?;
    let _ = sink.finish().await;
    let _ = sink.close().await;
    Ok(handshake.elapsed)
}

/// The sending half of a session.
pub struct QwenSink {
    inner: SplitSink<WsStream, Message>,
    task_id: String,
}

impl QwenSink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        Ok(self.inner.send(Message::Binary(pcm.into())).await?)
    }

    /// Tells the service the audio is over; it answers with the final result and `task-finished`.
    pub async fn finish(&mut self) -> Result<(), SendError> {
        let frame = protocol::finish_task(&self.task_id);
        Ok(self.inner.send(Message::Text(frame.into())).await?)
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
pub struct QwenStream {
    inner: SplitStream<WsStream>,
    /// The sentences that are over; the service only reports the current one.
    finished_text: String,
    received_any: bool,
    done: bool,
}

impl QwenStream {
    /// Next event; `None` after `Closed` or `Failed` has been returned.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        if self.done {
            return None;
        }
        loop {
            let event = match self.inner.next().await {
                Some(Ok(Message::Text(text))) => match protocol::parse(&text) {
                    Ok(Frame::Started | Frame::Heartbeat) => continue,
                    Ok(Frame::Sentence { text, end }) => {
                        let full = format!("{}{}", self.finished_text, text);
                        if end {
                            self.finished_text = full.clone();
                        }
                        AsrEvent::Server(ServerMsg::Result { text: full })
                    }
                    Ok(Frame::Finished) => AsrEvent::Server(ServerMsg::Finish),
                    Ok(Frame::Failed { code, message }) => AsrEvent::Server(ServerMsg::Error {
                        code: -1,
                        message: format!("{code}: {message}"),
                    }),
                    Ok(Frame::Unknown(event)) => AsrEvent::Server(ServerMsg::Unknown { event }),
                    Err(_) => AsrEvent::Garbage(text.chars().take(200).collect()),
                },
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
