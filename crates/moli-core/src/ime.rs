//! The Doubao IME backend: anonymous speech recognition and text cleanup
//! through the private API of the Doubao input method. Credentials, frames
//! and text cleanup come from the vendored `doubao-ime` crate
//! (`vendor/doubao-ime-rs`); the recognition protocol is driven here.
//!
//! Needs no login. Credentials (app_key → sami_token) are fetched at run
//! time, kept in memory only, and refetched when stale or refused.
//!
//! Protocol: open a socket, `StartTask`, then any number of sessions
//! (`StartSession`, audio, `FinishSession` → `SessionFinished`) on that task.
//! Opening takes seconds, starting a session on an open task a few hundred
//! ms, so a task is kept open between sessions (see [`ImeClient::warm`]).
//!
//! The service splits speech into segments. Each result carries the
//! segment's text so far, its `index` and its `start_time`; the full text is
//! the segments in order. A pause starts a segment with the next index, but
//! after about 24 s of speech without one the service starts a segment with
//! the same index: only the start time tells it apart.
//!
//! The service routes sessions by device id, and some ids are never routed:
//! the session starts fine and fails as soon as audio arrives ("service
//! discovery failure"). The warm-up checks the id with a little silence;
//! a session that runs into it anyway is replayed under a new device id.

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub use doubao_ime;
use doubao_ime::config::{
    AID, ASR_CHUNK_BYTES, ASR_NAMESPACE, ASR_RESOURCE_ID, DEVICE_PLATFORM, UA,
};
use doubao_ime::proto::{WebSocketRequest, WebSocketResponse, event};
use doubao_ime::{Client, ClientConfig, Credentials, ErrorKind};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::USER_AGENT;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::asr::client::{
    ConnectError, Handshake, SendError, install_crypto_provider, tcp_connect,
};
use crate::asr::{AsrEvent, ServerMsg};

/// Credentials are renewed this long before their token expires (it lasts
/// 24 h), or this long after fetching if the token's expiry is unreadable.
const RENEW_EARLY: Duration = Duration::from_secs(10 * 60);
const RENEW_BLIND: Duration = Duration::from_secs(30 * 60);
/// The service keeps a connection for 2 h (we ask for that); a kept-open one
/// is replaced before.
const CONN_LIFETIME: Duration = Duration::from_secs(110 * 60);
/// How long to wait for the final result after the audio is over, counted
/// again from each new text (on a slow network the audio is still arriving).
/// The session machine has its own, longer finalize timer.
const FINISH_IDLE: Duration = Duration::from_secs(5);
/// New sessions (and device ids) to try per recording when the connection
/// or session fails.
const MAX_REPLAYS: u32 = 3;
/// Audio is resent from this long before the unfinished segment's start.
const REWIND_MARGIN: f64 = 0.3;
/// 16 kHz mono s16le.
const BYTES_PER_SECOND: f64 = 32_000.0;
/// A kept-open task answers `StartSession` in about 300 ms. One that takes
/// longer is probably dead (the Mac slept, the network changed): open a new one.
const WARM_START: Duration = Duration::from_millis(1500);
/// Budget for opening and checking a connection in the background.
const WARM_TIMEOUT: Duration = Duration::from_secs(15);
/// Retry delays for the warm-up when the service cannot be reached.
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A kept-open connection that dies sooner than this is not replaced right away.
const STEADY: Duration = Duration::from_secs(60);
/// How often a kept-open connection is pinged, as the IME does.
const PING_EVERY: Duration = Duration::from_secs(20);
/// A kept-open connection that has sent nothing (not even a pong or the
/// service's own pings) for this long is dead.
const SILENT_LIMIT: Duration = Duration::from_secs(30);
/// What the IME sends: it sets punctuation and leaves the rest (silence
/// timeouts, sentence length, …) to the service.
const START_SESSION_PAYLOAD: &str = r#"{"extra":{"enable_punctuation":true}}"#;
/// The IME release this client presents itself as (`UA` says 1.0.1).
pub const IME_VERSION_CODE: u64 = 1_000_103;
const VERSION_URL: &str = "https://ime.doubao.com/api/v1/version/list";
/// Silence sent to check a device id: 100 ms.
const PROBE_BYTES: usize = 3200;
/// An unroutable device id fails within milliseconds of the first audio.
const PROBE_WAIT: Duration = Duration::from_secs(1);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Listener = Box<dyn Fn(&str) + Send + Sync>;

/// Handle to the IME service. Cheap to clone; clones share the device id,
/// the credential cache and the kept-open connection.
#[derive(Clone)]
pub struct ImeClient {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    device_id: Mutex<String>,
    /// With when to renew them.
    cached: Mutex<Option<(Credentials, Instant)>>,
    on_new_device_id: Mutex<Option<Listener>>,
    /// Keep a connection open between sessions.
    keep_warm: AtomicBool,
    /// The kept-open connection.
    slot: Mutex<Option<Parked>>,
    next_parked: AtomicU64,
    /// Sessions connecting or running.
    busy: AtomicUsize,
    /// Wakes the warm-up: something it waits for changed.
    wake: Notify,
    /// The device id the service is known to route.
    routable: Mutex<Option<String>>,
}

impl ImeClient {
    /// A client that presents `device_id` until the service refuses it.
    pub fn new(device_id: impl Into<String>) -> Result<Self, String> {
        Self::from_config(ClientConfig {
            device_id: Some(device_id.into()),
            ..ClientConfig::default()
        })
    }

    /// Full control over endpoints and timeouts (tests point these at a mock).
    pub fn from_config(config: ClientConfig) -> Result<Self, String> {
        install_crypto_provider();
        let device_id = config.device_id.clone().unwrap_or_else(Self::new_device_id);
        let client = Client::from_config(config).map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                device_id: Mutex::new(device_id),
                cached: Mutex::default(),
                on_new_device_id: Mutex::default(),
                keep_warm: AtomicBool::new(false),
                slot: Mutex::default(),
                next_parked: AtomicU64::new(0),
                busy: AtomicUsize::new(0),
                wake: Notify::new(),
                routable: Mutex::default(),
            }),
        })
    }

    /// A fresh random device id in the format the IME uses.
    pub fn new_device_id() -> String {
        // 16 decimal digits, like the real client.
        let n = uuid::Uuid::new_v4().as_u128() % 9_000_000_000_000_000 + 1_000_000_000_000_000;
        n.to_string()
    }

    pub fn device_id(&self) -> String {
        self.inner.device_id.lock().unwrap().clone()
    }

    /// Called with the new id whenever the client switches to one, so it can be saved.
    pub fn on_new_device_id(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.inner.on_new_device_id.lock().unwrap() = Some(Box::new(f));
    }

    /// Uses these credentials until they go stale or are refused.
    pub fn set_credentials(&self, creds: Credentials) {
        let renew_at = renew_at(&creds.sami_token);
        *self.inner.cached.lock().unwrap() = Some((creds, renew_at));
    }

    /// Forgets the cached credentials; the next connect fetches new ones.
    pub fn invalidate(&self) {
        self.inner.cached.lock().unwrap().take();
    }

    /// Fetches credentials now so the next connect does not wait for them.
    pub async fn prefetch(&self) {
        if let Err(e) = self.credentials().await {
            tracing::warn!("prefetching IME credentials: {e}");
        }
    }

    async fn credentials(&self) -> Result<(Credentials, Instant), doubao_ime::Error> {
        if let Some((creds, renew_at)) = self.inner.cached.lock().unwrap().as_ref()
            && Instant::now() < *renew_at
        {
            return Ok((creds.clone(), *renew_at));
        }
        let t = Instant::now();
        let device_id = self.device_id();
        let client = &self.inner.client;
        let app_key = client.fetch_app_key(&device_id).await?;
        let sami_token = client.fetch_sami_token(&app_key, &device_id).await?;
        let creds = Credentials {
            device_id,
            app_key,
            sami_token,
            ticket: None,
            ticket_exp: None,
        };
        let renew_at = renew_at(&creds.sami_token);
        tracing::debug!(
            ms = t.elapsed().as_millis(),
            renew_in_s = renew_at.saturating_duration_since(Instant::now()).as_secs(),
            "fetched IME credentials"
        );
        *self.inner.cached.lock().unwrap() = Some((creds.clone(), renew_at));
        Ok((creds, renew_at))
    }

    /// Switches to a new device id; credentials and the connection are renewed for it.
    fn rotate_device_id(&self) {
        let id = Self::new_device_id();
        tracing::info!("switching to a new IME device id");
        *self.inner.device_id.lock().unwrap() = id.clone();
        self.invalidate();
        self.inner.slot.lock().unwrap().take();
        if let Some(f) = self.inner.on_new_device_id.lock().unwrap().as_ref() {
            f(&id);
        }
    }

    fn forget_if_refused(&self, f: &Failure) {
        if matches!(f.kind, Kind::Refused | Kind::Unroutable) {
            self.invalidate();
        }
    }

    /// Opens a recognition session; the whole thing, credentials included,
    /// has to fit in `timeout`.
    pub async fn connect(
        &self,
        timeout: Duration,
    ) -> Result<(ImeSink, ImeStream, Handshake), ConnectError> {
        let t = Instant::now();
        let busy = Busy::new(self);
        let conn = match tokio::time::timeout(timeout, self.session()).await {
            Err(_) => return Err(ConnectError::Timeout),
            Ok(Err(f)) => {
                self.forget_if_refused(&f);
                return Err(match f.kind {
                    Kind::Timeout => ConnectError::Timeout,
                    _ => ConnectError::Transient(f.msg),
                });
            }
            Ok(Ok(conn)) => conn,
        };
        tracing::info!(ms = t.elapsed().as_millis(), "IME session started");
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let driver = Driver {
            conn,
            transcript: Transcript::default(),
            replay: Replay::new(timeout),
            events: event_tx,
            ime: self.clone(),
            news: Instant::now(),
            _busy: busy,
        };
        tokio::spawn(driver.run(cmd_rx));
        let handshake = Handshake {
            elapsed: t.elapsed(),
            set_cookies: Vec::new(),
        };
        Ok((
            ImeSink { tx: cmd_tx },
            ImeStream { rx: event_rx },
            handshake,
        ))
    }

    /// A connection with a session started: the kept-open one if it answers
    /// quickly, else a new one.
    async fn session(&self) -> Result<Conn, Failure> {
        if let Some(mut conn) = self.take_warm().await {
            match tokio::time::timeout(WARM_START, conn.start_session()).await {
                Ok(Ok(())) => return Ok(conn),
                Ok(Err(f)) => {
                    tracing::info!("kept-open IME connection failed: {}", f.msg);
                    self.forget_if_refused(&f);
                }
                Err(_) => tracing::info!("kept-open IME connection did not answer"),
            }
        }
        let mut conn = Conn::open(self).await?;
        conn.start_session().await?;
        Ok(conn)
    }

    /// Keep a connection open between sessions, or stop doing so. Takes
    /// effect only while [`warm`](Self::warm) runs.
    pub fn set_keep_warm(&self, on: bool) {
        self.inner.keep_warm.store(on, Ordering::SeqCst);
        if !on {
            self.inner.slot.lock().unwrap().take();
        }
        self.inner.wake.notify_one();
    }

    /// Whether a connection is open and waiting for the next session.
    pub fn is_warm(&self) -> bool {
        self.inner.slot.lock().unwrap().is_some()
    }

    /// Keeps a connection open, with the device id checked, whenever
    /// [`set_keep_warm`](Self::set_keep_warm) is on and no session runs.
    /// Runs forever; spawn it once.
    pub async fn warm(self) {
        let mut backoff = BACKOFF_MIN;
        loop {
            while !self.wants_warm() {
                self.inner.wake.notified().await;
            }
            match tokio::time::timeout(WARM_TIMEOUT, self.open_checked()).await {
                Ok(Ok(conn)) => {
                    tracing::info!("IME connection ready");
                    self.park(conn);
                    backoff = BACKOFF_MIN;
                    continue;
                }
                Ok(Err(f)) => {
                    tracing::warn!("warming up an IME connection: {}", f.msg);
                    self.forget_if_refused(&f);
                }
                Err(_) => tracing::warn!("warming up an IME connection: timed out"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    fn wants_warm(&self) -> bool {
        self.inner.keep_warm.load(Ordering::SeqCst)
            && self.inner.busy.load(Ordering::SeqCst) == 0
            && !self.is_warm()
    }

    /// A new connection, with the device id checked unless that was done before.
    async fn open_checked(&self) -> Result<Conn, Failure> {
        let mut rotations = 0;
        loop {
            let mut conn = Conn::open(self).await?;
            let id = conn.creds.device_id.clone();
            if self.inner.routable.lock().unwrap().as_deref() == Some(id.as_str()) {
                return Ok(conn);
            }
            match conn.probe().await {
                Ok(()) => {
                    *self.inner.routable.lock().unwrap() = Some(id);
                    return Ok(conn);
                }
                Err(f) if f.kind == Kind::Unroutable && rotations < MAX_REPLAYS => {
                    tracing::warn!("the IME service cannot route this device id");
                    rotations += 1;
                    self.rotate_device_id();
                }
                Err(f) => return Err(f),
            }
        }
    }

    /// Keeps `conn` for the next session, reading it meanwhile so pings get answered.
    fn park(&self, conn: Conn) {
        if !self.inner.keep_warm.load(Ordering::SeqCst) || conn.creds.device_id != self.device_id()
        {
            return;
        }
        let id = self.inner.next_parked.fetch_add(1, Ordering::SeqCst);
        let (take, taken) = oneshot::channel();
        // Replacing a parked connection drops it.
        *self.inner.slot.lock().unwrap() = Some(Parked { id, take });
        tokio::spawn(keep(conn, taken, Arc::downgrade(&self.inner), id));
    }

    async fn take_warm(&self) -> Option<Conn> {
        let parked = self.inner.slot.lock().unwrap().take()?;
        let (tx, rx) = oneshot::channel();
        parked.take.send(tx).ok()?;
        let conn = rx.await.ok()?;
        (conn.creds.device_id == self.device_id()).then_some(conn)
    }

    /// Rewrites spoken text as written text. `None` when the service fails,
    /// takes longer than `timeout`, or answers with nothing.
    pub async fn organize(&self, text: &str, timeout: Duration) -> Option<String> {
        let t = Instant::now();
        match tokio::time::timeout(timeout, self.inner.client.organize(text)).await {
            Err(_) => {
                tracing::warn!("organizing timed out");
                None
            }
            Ok(Err(e)) => {
                tracing::warn!("organizing failed: {e}");
                None
            }
            Ok(Ok(out)) => {
                let content = out.content.trim();
                tracing::info!(
                    ms = t.elapsed().as_millis(),
                    no_rewrite = out.no_rewrite,
                    "organized"
                );
                (!content.is_empty()).then(|| content.to_string())
            }
        }
    }
}

/// The latest IME release, `(name, code)`, if it is newer than the one this
/// client presents itself as: the service may one day turn old ones away.
/// `None` when it is not newer or cannot be told.
pub async fn newer_ime_version(timeout: Duration) -> Option<(String, u64)> {
    #[derive(serde::Deserialize)]
    struct Reply {
        code: i64,
        data: Option<Data>,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(default)]
        list: Vec<Release>,
    }
    #[derive(serde::Deserialize)]
    struct Release {
        version_name: Option<String>,
        version_code: Option<u64>,
    }
    install_crypto_provider();
    let aid = AID.to_string();
    let code = IME_VERSION_CODE.to_string();
    let query = [
        ("aid", aid.as_str()),
        ("platform", "macos"),
        ("channel", "release"),
        ("version_code", code.as_str()),
    ];
    let reply: Reply = async {
        reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(UA)
            .build()?
            .get(VERSION_URL)
            .query(&query)
            .send()
            .await?
            .json()
            .await
    }
    .await
    .inspect_err(|e| tracing::debug!("IME version check failed: {e}"))
    .ok()?;
    if reply.code != 0 {
        tracing::debug!(code = reply.code, "IME version check refused");
        return None;
    }
    let latest = reply
        .data?
        .list
        .into_iter()
        .filter_map(|r| Some((r.version_name.unwrap_or_default(), r.version_code?)))
        .max_by_key(|(_, code)| *code)?;
    (latest.1 > IME_VERSION_CODE).then_some(latest)
}

/// When to renew credentials with this token: shortly before the expiry
/// written in it (a JWT), measured from now so the Mac's clock does not matter.
fn renew_at(token: &str) -> Instant {
    use base64::Engine as _;
    let lifetime = token
        .split('.')
        .nth(1)
        .and_then(|claims| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims.trim_end_matches('='))
                .ok()
        })
        .and_then(|json| serde_json::from_slice::<serde_json::Value>(&json).ok())
        .and_then(|claims| Some(claims.get("exp")?.as_i64()? - claims.get("iat")?.as_i64()?))
        .and_then(|secs| u64::try_from(secs).ok())
        .map(Duration::from_secs);
    let renew_in = match lifetime {
        Some(l) if l > RENEW_EARLY * 2 => l - RENEW_EARLY,
        Some(l) => l / 2,
        None => RENEW_BLIND,
    };
    Instant::now() + renew_in
}

/// A parked connection: send it a channel to get the connection back.
struct Parked {
    id: u64,
    take: oneshot::Sender<oneshot::Sender<Conn>>,
}

/// Holds a parked connection until a session takes it, it dies, or its
/// credentials go stale; then lets the warm-up open another.
async fn keep(
    mut conn: Conn,
    taken: oneshot::Receiver<oneshot::Sender<Conn>>,
    inner: Weak<Inner>,
    id: u64,
) {
    let parked_at = Instant::now();
    let stale = tokio::time::sleep_until(conn.renew_at.min(conn.opened + CONN_LIFETIME).into());
    let mut ping = tokio::time::interval_at((parked_at + PING_EVERY).into(), PING_EVERY);
    tokio::pin!(stale, taken);
    let why = loop {
        tokio::select! {
            reply = &mut taken => {
                if let Ok(reply) = reply {
                    let _ = reply.send(conn);
                }
                return;
            }
            _ = &mut stale => break "due for renewal".to_string(),
            _ = ping.tick() => {
                if conn.heard.elapsed() > SILENT_LIMIT {
                    break "no answer to pings".to_string();
                }
                if let Err(f) = conn.ws.send(Message::Ping(Default::default())).await {
                    break f.to_string();
                }
            }
            // Results of the check session may still trickle in; ignore them.
            frame = conn.recv() => if let Err(f) = frame {
                break f.msg;
            },
        }
    };
    tracing::info!("kept-open IME connection closed: {why}");
    drop(conn);
    if let Some(strong) = inner.upgrade() {
        let mut slot = strong.slot.lock().unwrap();
        if slot.as_ref().is_some_and(|p| p.id == id) {
            slot.take();
        }
    }
    // One that did not last: something is off, so do not hammer the service.
    if parked_at.elapsed() < STEADY {
        tokio::time::sleep(STEADY / 6).await;
    }
    if let Some(inner) = inner.upgrade() {
        inner.wake.notify_one();
    }
}

/// Counts a session as running while alive; the warm-up waits for none.
struct Busy(ImeClient);

impl Busy {
    fn new(ime: &ImeClient) -> Self {
        ime.inner.busy.fetch_add(1, Ordering::SeqCst);
        Self(ime.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.inner.busy.fetch_sub(1, Ordering::SeqCst);
        self.0.inner.wake.notify_one();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Socket or HTTP trouble; the credentials are not to blame.
    Network,
    Timeout,
    /// The service said no: a failure event or a refused handshake.
    Refused,
    /// The service cannot route sessions for this device id.
    Unroutable,
}

#[derive(Debug)]
struct Failure {
    kind: Kind,
    msg: String,
}

impl Failure {
    fn network(e: impl std::fmt::Display) -> Self {
        Self {
            kind: Kind::Network,
            msg: e.to_string(),
        }
    }

    fn refused(e: impl std::fmt::Display) -> Self {
        Self {
            kind: Kind::Refused,
            msg: e.to_string(),
        }
    }

    fn event(resp: &WebSocketResponse) -> Self {
        Self {
            kind: if resp.status_code == 50_700_000 {
                Kind::Unroutable
            } else {
                Kind::Refused
            },
            msg: format!(
                "{}: [{}] {}",
                resp.event, resp.status_code, resp.status_text
            ),
        }
    }
}

impl From<doubao_ime::Error> for Failure {
    fn from(e: doubao_ime::Error) -> Self {
        let kind = match &e {
            doubao_ime::Error::Timeout(_) => Kind::Timeout,
            e if matches!(e.kind(), ErrorKind::Network) => Kind::Network,
            _ => Kind::Refused,
        };
        Self {
            kind,
            msg: e.to_string(),
        }
    }
}

/// A socket with a task started: ready for sessions, one at a time.
struct Conn {
    ws: Ws,
    creds: Credentials,
    /// When to renew the credentials.
    renew_at: Instant,
    opened: Instant,
    /// When the service last sent anything.
    heard: Instant,
}

impl Conn {
    async fn open(ime: &ImeClient) -> Result<Self, Failure> {
        let (creds, renew_at) = ime.credentials().await?;
        let config = ime.inner.client.config();
        let mut url = Url::parse(&config.endpoints.asr_ws).map_err(Failure::refused)?;
        url.query_pairs_mut()
            .append_pair("app_key", &creds.app_key)
            .append_pair("aid", &AID.to_string())
            .append_pair("device_id", &creds.device_id)
            .append_pair("device_platform", DEVICE_PLATFORM);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(Failure::refused)?;
        let ua = config.user_agent.as_deref().unwrap_or(UA);
        let headers = request.headers_mut();
        headers.insert("Proto-Version", HeaderValue::from_static("v2"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(ua).map_err(Failure::refused)?,
        );
        headers.insert(
            "X-Api-Resource-Id",
            HeaderValue::from_static(ASR_RESOURCE_ID),
        );
        // What the IME asks for: pings every 20 s, the connection kept for 2 h.
        headers.insert("x-custom-keepalive", HeaderValue::from_static("true"));
        headers.insert("x-keepalive-interval", HeaderValue::from_static("20"));
        headers.insert("x-keepalive-timeout", HeaderValue::from_static("7200"));

        let t = Instant::now();
        let host = url.host_str().ok_or_else(|| Failure::refused("no host"))?;
        let port = url.port_or_known_default().unwrap_or(443);
        let tcp = tcp_connect(host, port).await.map_err(Failure::network)?;
        let ws =
            match tokio_tungstenite::client_async_tls_with_config(request, tcp, None, None).await {
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
            renew_at,
            opened: Instant::now(),
            heard: Instant::now(),
        };
        conn.send(conn.request(event::START_TASK)).await?;
        conn.expect(event::TASK_STARTED).await?;
        tracing::debug!(ms = t.elapsed().as_millis(), "IME task started");
        Ok(conn)
    }

    fn request(&self, event: &str) -> WebSocketRequest {
        WebSocketRequest::new(event).appkey(&self.creds.app_key)
    }

    async fn send(&mut self, req: WebSocketRequest) -> Result<(), Failure> {
        self.ws
            .send(Message::Binary(req.to_bytes().into()))
            .await
            .map_err(Failure::network)
    }

    async fn start_session(&mut self) -> Result<(), Failure> {
        let req = self
            .request(event::START_SESSION)
            .token(&self.creds.sami_token)
            .payload(START_SESSION_PAYLOAD)
            .namespace(ASR_NAMESPACE);
        self.send(req).await?;
        self.expect(event::SESSION_STARTED).await?;
        Ok(())
    }

    /// Sends 16 kHz mono s16le PCM.
    async fn audio(&mut self, pcm: &[u8]) -> Result<(), Failure> {
        for chunk in pcm.chunks(ASR_CHUNK_BYTES) {
            let req = self.request(event::TASK_REQUEST).payload("{}").audio(chunk);
            self.send(req).await?;
        }
        Ok(())
    }

    async fn finish_session(&mut self) -> Result<(), Failure> {
        let req = self
            .request(event::FINISH_SESSION)
            .token(&self.creds.sami_token);
        self.send(req).await
    }

    /// The next frame from the service; failure events are errors.
    /// Cancel-safe.
    async fn recv(&mut self) -> Result<WebSocketResponse, Failure> {
        loop {
            let msg = self.ws.next().await;
            if matches!(msg, Some(Ok(_))) {
                self.heard = Instant::now();
            }
            match msg {
                None => return Err(Failure::network("connection closed")),
                Some(Err(e)) => return Err(Failure::network(e)),
                Some(Ok(Message::Binary(data))) => {
                    let resp = WebSocketResponse::from_bytes(&data).map_err(Failure::refused)?;
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
    async fn expect(&mut self, event: &str) -> Result<WebSocketResponse, Failure> {
        loop {
            let resp = self.recv().await?;
            if resp.event == event {
                return Ok(resp);
            }
        }
    }

    /// Checks the device id with a session of silence, which an unroutable
    /// id fails at once. Leaves the task ready for the next session.
    async fn probe(&mut self) -> Result<(), Failure> {
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

/// Results whose start times are this close belong to the same segment;
/// the service moves a segment's start by up to half a second as it goes.
const SEGMENT_START_SLACK: f64 = 1.5;

/// The text of a session, put together from its segments.
#[derive(Default)]
struct Transcript {
    /// In order of index, then start time.
    segments: Vec<Segment>,
    /// Added to indices, so that a follow-up session continues after the
    /// segments of the one before.
    base: i64,
}

struct Segment {
    index: i64,
    /// Seconds into the session, as first reported.
    start: Option<f64>,
    text: String,
    /// Being heard again in a new session; shown until that has results.
    stale: bool,
}

impl Segment {
    fn is(&self, index: i64, start: Option<f64>) -> bool {
        !self.stale
            && self.index == index
            && match (self.start, start) {
                (Some(a), Some(b)) => (a - b).abs() < SEGMENT_START_SLACK,
                _ => true,
            }
    }
}

impl Transcript {
    /// Takes in a result frame's payload; the full text if it changed.
    fn update(&mut self, payload: &str) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(payload).ok()?;
        let mut changed = false;
        for result in value.get("results")?.as_array()? {
            let Some(text) = result
                .get("text")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
            else {
                continue;
            };
            let index = self.base + result.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
            let start = result.get("start_time").and_then(|t| t.as_f64());
            self.segments.retain(|s| !s.stale);
            match self.segments.iter_mut().find(|s| s.is(index, start)) {
                Some(segment) if segment.text == text => {}
                Some(segment) => {
                    segment.text = text.to_string();
                    changed = true;
                }
                None => {
                    let at = self.segments.partition_point(|s| {
                        (s.index, s.start.unwrap_or(0.0)) <= (index, start.unwrap_or(0.0))
                    });
                    self.segments.insert(
                        at,
                        Segment {
                            index,
                            start,
                            text: text.to_string(),
                            stale: false,
                        },
                    );
                    changed = true;
                }
            }
        }
        changed.then(|| self.text())
    }

    /// The session broke off: its last segment may be unfinished, so a new
    /// session is to hear it again. The second that segment starts at in
    /// this session (0 to hear the whole session again).
    fn rewind(&mut self) -> f64 {
        let current = |s: &Segment| s.index >= self.base && !s.stale;
        let from = match self.segments.iter().rposition(current) {
            Some(last) if self.segments[last].start.is_some() => {
                self.segments[last].stale = true;
                self.segments[last].start.unwrap_or(0.0)
            }
            _ => {
                for s in self.segments.iter_mut().filter(|s| s.index >= self.base) {
                    s.stale = true;
                }
                0.0
            }
        };
        self.next_session();
        from
    }

    /// Later results belong to a new session, whose indices start over.
    fn next_session(&mut self) {
        self.base = self.segments.iter().map(|s| s.index + 1).max().unwrap_or(0);
    }

    fn text(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            let segment = segment.text.as_str();
            // Words of Latin script need a space between segments.
            if out.ends_with(|c: char| c.is_ascii_alphanumeric())
                && segment.starts_with(|c: char| c.is_ascii_alphanumeric())
            {
                out.push(' ');
            }
            out.push_str(segment);
        }
        out
    }
}

enum Cmd {
    Audio(Vec<u8>),
    Finish,
    Close,
}

pub struct ImeSink {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl ImeSink {
    /// Queues a chunk of 16 kHz mono s16le PCM.
    pub fn audio(&mut self, pcm: Vec<u8>) -> Result<(), SendError> {
        self.send(Cmd::Audio(pcm))
    }

    /// Ends the audio; the stream then yields the final result and `Finish`.
    pub fn finish(&mut self) -> Result<(), SendError> {
        self.send(Cmd::Finish)
    }

    pub fn close(&mut self) -> Result<(), SendError> {
        self.send(Cmd::Close)
    }

    fn send(&self, cmd: Cmd) -> Result<(), SendError> {
        self.tx.send(cmd).map_err(|_| SendError::Closed)
    }
}

pub struct ImeStream {
    rx: mpsc::UnboundedReceiver<AsrEvent>,
}

impl ImeStream {
    /// Next event; `None` once the session is over.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        self.rx.recv().await
    }
}

/// Owns a session: feeds it audio and turns its results into [`AsrEvent`]s.
struct Driver {
    conn: Conn,
    transcript: Transcript,
    replay: Replay,
    events: mpsc::UnboundedSender<AsrEvent>,
    ime: ImeClient,
    /// When the text last changed.
    news: Instant,
    _busy: Busy,
}

impl Driver {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Audio(pcm)) => {
                        self.replay.record(&pcm);
                        if let Err(f) = self.conn.audio(&pcm).await
                            && let Err(f) = self.recover(f).await
                        {
                            return self.fail(f);
                        }
                    }
                    Some(Cmd::Finish) => return self.finish().await,
                    // Dropping the connection closes it.
                    Some(Cmd::Close) | None => return,
                },
                frame = self.conn.recv() => {
                    let outcome = match frame {
                        Ok(resp) if self.take_in(&resp) => self.resume().await,
                        Ok(_) => Ok(()),
                        Err(f) => self.recover(f).await,
                    };
                    if let Err(f) = outcome {
                        return self.fail(f);
                    }
                }
            }
        }
    }

    /// Handles a frame; true when it ends the session.
    fn take_in(&mut self, resp: &WebSocketResponse) -> bool {
        tracing::trace!(event = %resp.event, payload = %resp.payload, "IME frame");
        if !resp.payload.is_empty()
            && let Some(text) = self.transcript.update(&resp.payload)
        {
            self.news = Instant::now();
            let _ = self
                .events
                .send(AsrEvent::Server(ServerMsg::Result { text }));
        }
        resp.event == event::SESSION_FINISHED
    }

    /// The service ended the session while audio was still coming: carry on
    /// in a new one on the same task.
    async fn resume(&mut self) -> Result<(), Failure> {
        tracing::info!("the IME service ended the session early; starting another");
        self.transcript.next_session();
        self.replay.audio.clear();
        self.conn.start_session().await
    }

    async fn finish(mut self) {
        self.news = Instant::now();
        let mut finish_sent = false;
        loop {
            let deadline = tokio::time::Instant::from_std(self.news + FINISH_IDLE);
            let next = async {
                if !finish_sent {
                    self.conn.finish_session().await?;
                }
                self.conn.recv().await
            };
            match tokio::time::timeout_at(deadline, next).await {
                Ok(Ok(resp)) => {
                    finish_sent = true;
                    if !self.take_in(&resp) {
                        continue;
                    }
                    let text = self.transcript.text();
                    // Park before reporting, so the next session finds it.
                    self.ime.park(self.conn);
                    let _ = self
                        .events
                        .send(AsrEvent::Server(ServerMsg::Result { text }));
                    let _ = self.events.send(AsrEvent::Server(ServerMsg::Finish));
                    return;
                }
                Ok(Err(f)) => {
                    if let Err(f) = self.recover(f).await {
                        return self.fail(f);
                    }
                    finish_sent = false;
                }
                Err(_) => {
                    tracing::warn!("no final IME result in time; using the last one");
                    let text = self.transcript.text();
                    let _ = self
                        .events
                        .send(AsrEvent::Server(ServerMsg::Result { text }));
                    let _ = self.events.send(AsrEvent::Server(ServerMsg::Finish));
                    return;
                }
            }
        }
    }

    fn fail(self, f: Failure) {
        self.ime.forget_if_refused(&f);
        let _ = self.events.send(AsrEvent::Failed(f.msg));
    }

    /// Carries on in a new session, which first hears again the speech that
    /// the broken one may not have finished with; or gives the failure back.
    async fn recover(&mut self, mut f: Failure) -> Result<(), Failure> {
        let from = self.transcript.rewind();
        let skip = ((from - REWIND_MARGIN).max(0.0) * BYTES_PER_SECOND) as usize & !1;
        let skip = skip.min(self.replay.audio.len());
        self.replay.audio.drain(..skip);
        loop {
            self.ime.forget_if_refused(&f);
            if self.replay.replays >= MAX_REPLAYS {
                return Err(f);
            }
            self.replay.replays += 1;
            tracing::warn!(
                replay = self.replay.replays,
                resend_ms = self.replay.audio.len() as f64 / BYTES_PER_SECOND * 1000.0,
                "IME session failed; continuing in a new one: {}",
                f.msg
            );
            if f.kind == Kind::Unroutable {
                self.ime.rotate_device_id();
            }
            let audio = &self.replay.audio;
            let attempt = async {
                let mut conn = self.ime.session().await?;
                conn.audio(audio).await?;
                Ok(conn)
            };
            match tokio::time::timeout(self.replay.timeout, attempt).await {
                Ok(Ok(conn)) => {
                    self.conn = conn;
                    return Ok(());
                }
                Ok(Err(e)) => f = e,
                Err(_) => {
                    f = Failure {
                        kind: Kind::Timeout,
                        msg: "reconnecting timed out".into(),
                    }
                }
            }
        }
    }
}

/// The audio of the service's current session, to hear again in a new one
/// if that one breaks off.
struct Replay {
    timeout: Duration,
    /// From the session's start: result times count from here.
    audio: Vec<u8>,
    replays: u32,
}

impl Replay {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            audio: Vec::new(),
            replays: 0,
        }
    }

    fn record(&mut self, pcm: &[u8]) {
        self.audio.extend_from_slice(pcm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(index: Option<i64>, text: &str) -> String {
        let mut r = serde_json::json!({"text": text, "is_interim": true});
        if let Some(i) = index {
            r["index"] = i.into();
        }
        serde_json::json!({"results": [r]}).to_string()
    }

    #[test]
    fn segments_add_up() {
        let mut t = Transcript::default();
        assert_eq!(t.update(&result(Some(0), "你好")).as_deref(), Some("你好"));
        assert_eq!(
            t.update(&result(Some(0), "你好世界。")).as_deref(),
            Some("你好世界。")
        );
        // After a pause the next segment starts from scratch.
        assert_eq!(
            t.update(&result(Some(1), "今天")).as_deref(),
            Some("你好世界。今天")
        );
        assert_eq!(t.update(&result(Some(1), "今天")), None);
        assert_eq!(
            t.update(&result(Some(1), "今天天气不错。")).as_deref(),
            Some("你好世界。今天天气不错。")
        );
    }

    fn timed(index: i64, start: f64, text: &str) -> String {
        serde_json::json!({"results": [
            {"text": text, "index": index, "start_time": start, "is_interim": true}
        ]})
        .to_string()
    }

    // As seen on 2026-09-28: 24 s into speech without a pause, the service
    // sends an empty result, then a new segment with the same index.
    #[test]
    fn a_long_segment_is_split_by_start_time() {
        let mut t = Transcript::default();
        t.update(&timed(0, 0.0, "一二三"));
        t.update(&timed(0, 0.0, "一二三四五。"));
        t.update(&timed(0, 0.0, ""));
        assert_eq!(
            t.update(&timed(0, 23.194, "六")).as_deref(),
            Some("一二三四五。六")
        );
        assert_eq!(
            t.update(&timed(0, 23.61, "六七八")).as_deref(),
            Some("一二三四五。六七八")
        );
        assert_eq!(
            t.update(&timed(0, 23.359, "六七八九。")).as_deref(),
            Some("一二三四五。六七八九。")
        );
        // A late word on the first segment stays in its place.
        assert_eq!(
            t.update(&timed(0, 0.2, "一二三四五，")).as_deref(),
            Some("一二三四五，六七八九。")
        );
    }

    #[test]
    fn a_rewind_hears_the_last_segment_again() {
        let mut t = Transcript::default();
        t.update(&timed(0, 0.0, "一二。"));
        t.update(&timed(1, 3.0, "三四"));
        assert_eq!(t.rewind(), 3.0);
        // The old text stays until the new session has some.
        assert_eq!(t.text(), "一二。三四");
        assert_eq!(
            t.update(&timed(0, 0.3, "三四五")).as_deref(),
            Some("一二。三四五")
        );
        assert_eq!(
            t.update(&timed(0, 0.3, "三四五六。")).as_deref(),
            Some("一二。三四五六。")
        );
    }

    #[test]
    fn a_rewind_without_times_hears_the_whole_session_again() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "一。"));
        t.next_session();
        t.update(&result(Some(0), "二"));
        assert_eq!(t.rewind(), 0.0);
        assert_eq!(
            t.update(&result(Some(0), "二三")).as_deref(),
            Some("一。二三")
        );
    }

    #[test]
    fn no_index_means_the_first_segment() {
        let mut t = Transcript::default();
        t.update(&result(None, "你好"));
        assert_eq!(
            t.update(&result(None, "你好世界")).as_deref(),
            Some("你好世界")
        );
    }

    #[test]
    fn empty_and_foreign_payloads_change_nothing() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "你好"));
        assert_eq!(t.update(&result(Some(0), "")), None);
        assert_eq!(t.update("{}"), None);
        assert_eq!(t.update("not json"), None);
        assert_eq!(t.text(), "你好");
    }

    #[test]
    fn latin_segments_are_spaced() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "hello"));
        t.update(&result(Some(1), "world"));
        t.update(&result(Some(2), "你好"));
        assert_eq!(t.text(), "hello world你好");
    }

    #[test]
    fn a_follow_up_session_continues_the_text() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "一。"));
        t.update(&result(Some(1), "二。"));
        t.next_session();
        assert_eq!(
            t.update(&result(Some(0), "三。")).as_deref(),
            Some("一。二。三。")
        );
    }
}
