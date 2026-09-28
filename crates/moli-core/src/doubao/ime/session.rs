//! A running session: audio in through [`ImeSink`], events out through
//! [`ImeStream`], and a [`Driver`] in between that carries on in a new
//! session when the service breaks one off.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::client::ImeClient;
use super::connection::Conn;
use super::error::{Failure, Kind};
use super::pool::Busy;
use super::transcript::Transcript;
use super::wire::{Response, event};
use crate::asr::{AsrEvent, SendError, ServerMsg};

/// How long to wait for the final result after the audio is over, counted
/// again from each new text (on a slow network the audio is still arriving).
/// The session machine has its own, longer finalize timer.
const FINISH_IDLE: Duration = Duration::from_secs(5);
/// New sessions (and device ids) to try per recording when the connection
/// or session fails.
pub(super) const MAX_REPLAYS: u32 = 3;
/// Audio is resent from this long before the unfinished segment's start.
const REWIND_MARGIN: f64 = 0.3;
/// 16 kHz mono s16le.
const BYTES_PER_SECOND: f64 = 32_000.0;

pub(super) enum Cmd {
    Audio(Vec<u8>),
    Finish,
    Close,
}

/// The sending half of a session.
pub struct ImeSink {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl ImeSink {
    pub(super) fn new(tx: mpsc::UnboundedSender<Cmd>) -> Self {
        Self { tx }
    }

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

/// The receiving half of a session.
pub struct ImeStream {
    rx: mpsc::UnboundedReceiver<AsrEvent>,
}

impl ImeStream {
    pub(super) fn new(rx: mpsc::UnboundedReceiver<AsrEvent>) -> Self {
        Self { rx }
    }

    /// Next event; `None` once the session is over.
    pub async fn next(&mut self) -> Option<AsrEvent> {
        self.rx.recv().await
    }
}

/// Owns a session: feeds it audio and turns its results into [`AsrEvent`]s.
pub(super) struct Driver {
    pub conn: Conn,
    pub transcript: Transcript,
    pub replay: Replay,
    pub events: mpsc::UnboundedSender<AsrEvent>,
    pub ime: ImeClient,
    /// When the text last changed.
    pub news: Instant,
    pub _busy: Busy,
}

impl Driver {
    pub async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
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
    fn take_in(&mut self, resp: &Response) -> bool {
        tracing::trace!(event = %resp.event, payload = %resp.payload, "IME frame");
        if !resp.payload.is_empty()
            && let Some(text) = self.transcript.update(&resp.payload)
        {
            self.news = Instant::now();
            self.report(text);
        }
        resp.event == event::SESSION_FINISHED
    }

    fn report(&self, text: String) {
        let _ = self
            .events
            .send(AsrEvent::Server(ServerMsg::Result { text }));
    }

    fn report_final(&self) {
        self.report(self.transcript.text());
        let _ = self.events.send(AsrEvent::Server(ServerMsg::Finish));
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
                    // Park before reporting, so the next session finds it.
                    let text = self.transcript.text();
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
                    return self.report_final();
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
                Err(_) => f = Failure::timeout("reconnecting timed out"),
            }
        }
    }
}

/// The audio of the service's current session, to hear again in a new one
/// if that one breaks off.
pub(super) struct Replay {
    timeout: Duration,
    /// From the session's start: result times count from here.
    audio: Vec<u8>,
    replays: u32,
}

impl Replay {
    pub fn new(timeout: Duration) -> Self {
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
