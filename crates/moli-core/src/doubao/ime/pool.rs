//! Keeping a connection open between sessions, so the next one starts in
//! a few hundred ms instead of seconds.

use std::sync::Weak;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use futures_util::SinkExt;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

use super::client::{ImeClient, Inner};
use super::connection::{Conn, PING_EVERY, SILENT_LIMIT};
use super::error::{Failure, Kind};
use super::session::MAX_REPLAYS;

/// The service keeps a connection for 2 h (we ask for that); a kept-open one
/// is replaced before.
const CONN_LIFETIME: Duration = Duration::from_secs(110 * 60);
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

impl ImeClient {
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
    pub(super) fn park(&self, conn: Conn) {
        if !self.inner.keep_warm.load(Ordering::SeqCst) || conn.creds.device_id != self.device_id()
        {
            return;
        }
        let id = self.inner.next_parked.fetch_add(1, Ordering::SeqCst);
        let (take, taken) = oneshot::channel();
        // Replacing a parked connection drops it.
        *self.inner.slot.lock().unwrap() = Some(Parked { id, take });
        tokio::spawn(keep(
            conn,
            taken,
            std::sync::Arc::downgrade(&self.inner),
            id,
        ));
    }

    /// The kept-open connection with a session started, if it answers quickly.
    pub(super) async fn take_warm_started(&self) -> Option<Conn> {
        let mut conn = self.take_warm().await?;
        match tokio::time::timeout(WARM_START, conn.start_session()).await {
            Ok(Ok(())) => return Some(conn),
            Ok(Err(f)) => {
                tracing::info!("kept-open IME connection failed: {}", f.msg);
                self.forget_if_refused(&f);
            }
            Err(_) => tracing::info!("kept-open IME connection did not answer"),
        }
        None
    }

    async fn take_warm(&self) -> Option<Conn> {
        let parked = self.inner.slot.lock().unwrap().take()?;
        let (tx, rx) = oneshot::channel();
        parked.take.send(tx).ok()?;
        let conn = rx.await.ok()?;
        (conn.creds.device_id == self.device_id()).then_some(conn)
    }
}

/// A parked connection: send it a channel to get the connection back.
pub(super) struct Parked {
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
    let stale =
        tokio::time::sleep_until(conn.creds.renew_at.min(conn.opened + CONN_LIFETIME).into());
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
                if let Err(e) = conn.ws.send(Message::Ping(Default::default())).await {
                    break e.to_string();
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
pub(super) struct Busy(ImeClient);

impl Busy {
    pub fn new(ime: &ImeClient) -> Self {
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
