//! The handle apps hold: device id, credentials, and opening sessions.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc};

use super::connection::Conn;
use super::credentials::Credentials;
use super::error::{Failure, Kind};
use super::pool::{Busy, Parked};
use super::session::{Driver, ImeSink, ImeStream, Replay};
use super::transcript::Transcript;
use super::{Endpoints, USER_AGENT};
use crate::asr::{ConnectError, Handshake};
use crate::doubao::net;

/// Budget for each HTTP call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

type Listener = Box<dyn Fn(&str) + Send + Sync>;

/// Handle to the IME service. Cheap to clone; clones share the device id,
/// the credential cache and the kept-open connection.
#[derive(Clone)]
pub struct ImeClient {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub http: reqwest::Client,
    pub endpoints: Endpoints,
    pub device_id: Mutex<String>,
    pub credentials: Mutex<Option<Credentials>>,
    pub on_new_device_id: Mutex<Option<Listener>>,
    /// Keep a connection open between sessions.
    pub keep_warm: AtomicBool,
    /// The kept-open connection.
    pub slot: Mutex<Option<Parked>>,
    pub next_parked: AtomicU64,
    /// Sessions connecting or running.
    pub busy: AtomicUsize,
    /// Wakes the warm-up: something it waits for changed.
    pub wake: Notify,
    /// The device id the service is known to route.
    pub routable: Mutex<Option<String>>,
}

impl ImeClient {
    /// A client that presents `device_id` until the service refuses it.
    pub fn new(device_id: impl Into<String>) -> Self {
        Self::with_endpoints(device_id, Endpoints::default())
    }

    /// A client that talks to `endpoints` instead of the production hosts.
    pub fn with_endpoints(device_id: impl Into<String>, endpoints: Endpoints) -> Self {
        net::install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .expect("the HTTP client builds with static settings");
        Self {
            inner: Arc::new(Inner {
                http,
                endpoints,
                device_id: Mutex::new(device_id.into()),
                credentials: Mutex::default(),
                on_new_device_id: Mutex::default(),
                keep_warm: AtomicBool::new(false),
                slot: Mutex::default(),
                next_parked: AtomicU64::new(0),
                busy: AtomicUsize::new(0),
                wake: Notify::new(),
                routable: Mutex::default(),
            }),
        }
    }

    /// A fresh random device id in the format the IME uses: 16 decimal digits.
    pub fn new_device_id() -> String {
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

    /// Forgets the cached credentials; the next connect fetches new ones.
    pub fn invalidate(&self) {
        self.inner.credentials.lock().unwrap().take();
    }

    /// Fetches credentials now so the next connect does not wait for them.
    pub async fn prefetch(&self) {
        if let Err(f) = self.credentials().await {
            tracing::warn!("prefetching IME credentials: {}", f.msg);
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
        Ok((ImeSink::new(cmd_tx), ImeStream::new(event_rx), handshake))
    }

    /// A connection with a session started: the kept-open one if it answers
    /// quickly, else a new one.
    pub(super) async fn session(&self) -> Result<Conn, Failure> {
        if let Some(conn) = self.take_warm_started().await {
            return Ok(conn);
        }
        let mut conn = Conn::open(self).await?;
        conn.start_session().await?;
        Ok(conn)
    }

    /// The cached credentials while fresh, else new ones.
    pub(super) async fn credentials(&self) -> Result<Credentials, Failure> {
        if let Some(creds) = self.inner.credentials.lock().unwrap().as_ref()
            && creds.is_fresh()
        {
            return Ok(creds.clone());
        }
        let t = Instant::now();
        let device_id = self.device_id();
        let app_key = self.fetch_app_key(&device_id).await?;
        let sami_token = self.fetch_sami_token(&app_key, &device_id).await?;
        let creds = Credentials::new(device_id, app_key, sami_token);
        tracing::debug!(
            ms = t.elapsed().as_millis(),
            renew_in_s = creds
                .renew_at
                .saturating_duration_since(Instant::now())
                .as_secs(),
            "fetched IME credentials"
        );
        *self.inner.credentials.lock().unwrap() = Some(creds.clone());
        Ok(creds)
    }

    /// Switches to a new device id; credentials and the connection are renewed for it.
    pub(super) fn rotate_device_id(&self) {
        let id = Self::new_device_id();
        tracing::info!("switching to a new IME device id");
        *self.inner.device_id.lock().unwrap() = id.clone();
        self.invalidate();
        self.inner.slot.lock().unwrap().take();
        if let Some(f) = self.inner.on_new_device_id.lock().unwrap().as_ref() {
            f(&id);
        }
    }

    pub(super) fn forget_if_refused(&self, f: &Failure) {
        if f.blames_credentials() {
            self.invalidate();
        }
    }
}
