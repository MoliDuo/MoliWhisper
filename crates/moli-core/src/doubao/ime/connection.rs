//! A socket with a task started, and the frames that run sessions on it.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::USER_AGENT;
use tokio_tungstenite::tungstenite::{self};
use url::Url;

use super::client::ImeClient;
use super::credentials::Credentials;
use super::error::Failure;
use super::wire::{Request, Response, event};
use super::{AID, DEVICE_PLATFORM};
use crate::net::{self, WsStream};

const RESOURCE_ID: &str = "original.sami.ASR";
const NAMESPACE: &str = "ASR";
/// Audio goes up in frames of 200 ms.
const CHUNK_BYTES: usize = 6400;
/// What the IME sends: it sets punctuation and leaves the rest (silence
/// timeouts, sentence length, …) to the service.
const START_SESSION_PAYLOAD: &str = r#"{"extra":{"enable_punctuation":true}}"#;
/// How often a kept-open connection is pinged, as the IME does.
pub(super) const PING_EVERY: Duration = Duration::from_secs(20);
/// A kept-open connection that has sent nothing (not even a pong or the
/// service's own pings) for this long is dead.
pub(super) const SILENT_LIMIT: Duration = Duration::from_secs(30);
/// Silence sent to check a device id: 100 ms.
const PROBE_BYTES: usize = 3200;
/// An unroutable device id fails within milliseconds of the first audio.
const PROBE_WAIT: Duration = Duration::from_secs(1);

/// A socket with a task started: ready for sessions, one at a time.
pub(super) struct Conn {
    pub ws: WsStream,
    pub creds: Credentials,
    pub opened: Instant,
    /// When the service last sent anything.
    pub heard: Instant,
}

impl Conn {
    pub async fn open(ime: &ImeClient) -> Result<Self, Failure> {
        let creds = ime.credentials().await?;
        let mut url = Url::parse(&ime.inner.endpoints.asr_ws).map_err(Failure::refused)?;
        url.query_pairs_mut()
            .append_pair("app_key", &creds.app_key)
            .append_pair("aid", &AID.to_string())
            .append_pair("device_id", &creds.device_id)
            .append_pair("device_platform", DEVICE_PLATFORM);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(Failure::refused)?;
        let headers = request.headers_mut();
        headers.insert("Proto-Version", HeaderValue::from_static("v2"));
        headers.insert(USER_AGENT, HeaderValue::from_static(super::USER_AGENT));
        headers.insert("X-Api-Resource-Id", HeaderValue::from_static(RESOURCE_ID));
        // What the IME asks for: pings every 20 s, the connection kept for 2 h.
        headers.insert("x-custom-keepalive", HeaderValue::from_static("true"));
        headers.insert("x-keepalive-interval", HeaderValue::from_static("20"));
        headers.insert("x-keepalive-timeout", HeaderValue::from_static("7200"));

        let t = Instant::now();
        let ws = match net::connect_websocket(request).await {
            Ok((ws, _)) => ws,
            Err(tungstenite::Error::Http(response)) => {
                return Err(Failure::refused(format!(
                    "handshake rejected with HTTP {}",
                    response.status()
                )));
            }
            Err(e) => return Err(Failure::network(e)),
        };
        let mut conn = Self {
            ws,
            creds,
            opened: Instant::now(),
            heard: Instant::now(),
        };
        conn.send(conn.request(event::START_TASK)).await?;
        conn.expect(event::TASK_STARTED).await?;
        tracing::debug!(ms = t.elapsed().as_millis(), "IME task started");
        Ok(conn)
    }

    fn request(&self, event: &str) -> Request {
        Request::new(event).appkey(&self.creds.app_key)
    }

    async fn send(&mut self, req: Request) -> Result<(), Failure> {
        self.ws
            .send(Message::Binary(req.to_bytes().into()))
            .await
            .map_err(Failure::network)
    }

    pub async fn start_session(&mut self) -> Result<(), Failure> {
        let req = self
            .request(event::START_SESSION)
            .token(&self.creds.sami_token)
            .payload(START_SESSION_PAYLOAD)
            .namespace(NAMESPACE);
        self.send(req).await?;
        self.expect(event::SESSION_STARTED).await?;
        Ok(())
    }

    /// Sends 16 kHz mono s16le PCM.
    pub async fn audio(&mut self, pcm: &[u8]) -> Result<(), Failure> {
        for chunk in pcm.chunks(CHUNK_BYTES) {
            let req = self.request(event::TASK_REQUEST).payload("{}").audio(chunk);
            self.send(req).await?;
        }
        Ok(())
    }

    pub async fn finish_session(&mut self) -> Result<(), Failure> {
        let req = self
            .request(event::FINISH_SESSION)
            .token(&self.creds.sami_token);
        self.send(req).await
    }

    /// The next frame from the service; failure events are errors.
    /// Cancel-safe.
    pub async fn recv(&mut self) -> Result<Response, Failure> {
        loop {
            let msg = self.ws.next().await;
            if matches!(msg, Some(Ok(_))) {
                self.heard = Instant::now();
            }
            match msg {
                None => return Err(Failure::network("connection closed")),
                Some(Err(e)) => return Err(Failure::network(e)),
                Some(Ok(Message::Binary(data))) => {
                    let resp = Response::from_bytes(&data).map_err(Failure::refused)?;
                    if resp.is_failure() {
                        return Err(Failure::event(&resp));
                    }
                    return Ok(resp);
                }
                Some(Ok(Message::Close(_))) => {
                    return Err(Failure::network("connection closed by the service"));
                }
                // Pings are answered by tungstenite.
                Some(Ok(_)) => {}
            }
        }
    }

    /// Reads until `event`, skipping anything else (like late results).
    async fn expect(&mut self, event: &str) -> Result<Response, Failure> {
        loop {
            let resp = self.recv().await?;
            if resp.event == event {
                return Ok(resp);
            }
        }
    }

    /// Checks the device id with a session of silence, which an unroutable
    /// id fails at once. Leaves the task ready for the next session.
    pub async fn probe(&mut self) -> Result<(), Failure> {
        self.start_session().await?;
        self.audio(&[0; PROBE_BYTES]).await?;
        let listen = async {
            loop {
                self.recv().await?;
            }
        };
        let r: Result<Result<Infallible, Failure>, _> =
            tokio::time::timeout(PROBE_WAIT, listen).await;
        if let Ok(Err(f)) = r {
            return Err(f);
        }
        self.finish_session().await?;
        self.expect(event::SESSION_FINISHED).await?;
        Ok(())
    }
}
