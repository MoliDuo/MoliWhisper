//! WebSocket transport: connect with the login cookies, stream PCM up, read
//! JSON events down.

use std::io;
use std::net::SocketAddr;
use std::sync::Once;
use std::time::{Duration, Instant};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, SET_COOKIE, USER_AGENT};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use super::params::{self, Overrides};
use super::protocol::{self, FINISH_FRAME, ServerMsg};
use crate::creds::Credentials;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Delay before racing the next address family (RFC 8305 recommends 250 ms).
const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

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

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The upgrade was answered with a plain HTTP status. 403 means a missing
    /// `Origin` or a WAF block; with no cookies at all the server answers 200.
    /// An invalid session does not show up here: the handshake succeeds and the
    /// server rejects it afterwards (see [`protocol::is_session_rejected`]).
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

pub async fn connect(
    opts: &ConnectOptions,
) -> Result<(AsrSink, AsrStream, Handshake), ConnectError> {
    install_crypto_provider();
    let started = Instant::now();
    let (ws, set_cookies) = tokio::time::timeout(opts.timeout, handshake(opts))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    let (sink, stream) = ws.split();
    Ok((
        AsrSink { inner: sink },
        AsrStream {
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

async fn handshake(opts: &ConnectOptions) -> Result<(Ws, Vec<String>), ConnectError> {
    let transient = |e: &dyn std::fmt::Display| ConnectError::Transient(e.to_string());

    let mut request = opts
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| transient(&e))?;
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

    let host = opts
        .url
        .host_str()
        .ok_or_else(|| ConnectError::Transient("url has no host".into()))?;
    let port = opts.url.port_or_known_default().unwrap_or(443);
    let t = Instant::now();
    let tcp = tcp_connect(host, port).await.map_err(|e| transient(&e))?;
    tracing::debug!(peer = ?tcp.peer_addr().ok(), ms = t.elapsed().as_millis(), "tcp connected");

    let t = Instant::now();
    match tokio_tungstenite::client_async_tls_with_config(request, tcp, None, None).await {
        Ok((ws, response)) => {
            tracing::debug!(
                ms = t.elapsed().as_millis(),
                "tls + websocket handshake done"
            );
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
        Err(e) => Err(transient(&e)),
    }
}

fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Fails only if another provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Happy-eyeballs TCP connect: try addresses alternating between IPv6 and
/// IPv4, starting a new attempt every 250 ms or as soon as one fails.
async fn tcp_connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let t = Instant::now();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    tracing::debug!(?addrs, ms = t.elapsed().as_millis(), "resolved");
    let mut pending = interleave_families(addrs).into_iter().peekable();
    let mut attempts = JoinSet::new();
    let mut last_err = None;

    loop {
        if let Some(addr) = pending.next() {
            attempts.spawn(async move { TcpStream::connect(addr).await });
        } else if attempts.is_empty() {
            return Err(
                last_err.unwrap_or_else(|| io::Error::other(format!("no addresses for {host}")))
            );
        }
        let more = pending.peek().is_some();
        tokio::select! {
            Some(joined) = attempts.join_next() => match joined {
                Ok(Ok(stream)) => {
                    attempts.abort_all();
                    stream.set_nodelay(true)?;
                    return Ok(stream);
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(e) => last_err = Some(io::Error::other(e)),
            },
            _ = tokio::time::sleep(HAPPY_EYEBALLS_DELAY), if more => {}
        }
    }
}

/// Keeps the resolver's first family first, then alternates.
fn interleave_families(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return addrs;
    };
    let first_v6 = first.is_ipv6();
    let (mut a, mut b): (Vec<_>, Vec<_>) = addrs.into_iter().partition(|x| x.is_ipv6() == first_v6);
    let mut out = Vec::with_capacity(a.len() + b.len());
    a.reverse();
    b.reverse();
    while !a.is_empty() || !b.is_empty() {
        out.extend(a.pop());
        out.extend(b.pop());
    }
    out
}

#[derive(Debug, thiserror::Error)]
#[error("send failed: {0}")]
pub struct SendError(#[from] tungstenite::Error);

pub struct AsrSink {
    inner: SplitSink<Ws, Message>,
}

impl AsrSink {
    /// Sends a chunk of 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        Ok(self.inner.send(Message::Binary(pcm.into())).await?)
    }

    /// Tells the server the audio is over; it answers with the final result and `finish`.
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

pub struct AsrStream {
    inner: SplitStream<Ws>,
    received_any: bool,
    done: bool,
}

impl AsrStream {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_starting_with_first_family() {
        let v6a: SocketAddr = "[::1]:1".parse().unwrap();
        let v6b: SocketAddr = "[::2]:1".parse().unwrap();
        let v4a: SocketAddr = "1.1.1.1:1".parse().unwrap();
        let v4b: SocketAddr = "2.2.2.2:1".parse().unwrap();
        assert_eq!(
            interleave_families(vec![v4a, v4b, v6a, v6b]),
            vec![v4a, v6a, v4b, v6b]
        );
        assert_eq!(
            interleave_families(vec![v6a, v6b, v4a]),
            vec![v6a, v4a, v6b]
        );
        assert_eq!(interleave_families(vec![]), vec![]);
    }
}
