//! Runs the state machine: one actor task owns the [`Machine`] and turns its
//! effects into IO. Each session gets a pipe task that owns the microphone
//! and the connection, so a stalled socket never blocks the actor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use super::Outcome;
use super::machine::{Effect, Event, Machine, Phase, Sid, Timings};
use crate::asr::{AsrEvent, ConnectError, Handshake, ServerMsg};
use crate::audio::{AudioEvent, AudioInput};
use crate::qwen::{self, QwenSink, QwenStream};

/// How long a finished pipe may take to close the connection politely.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// What the controller needs from the app.
pub trait Env: Send + Sync + 'static {
    /// Where and how to connect, or `None` without an API key.
    fn connect_options(&self) -> Option<qwen::ConnectOptions>;
    fn start_audio(&self) -> AudioInput;
    /// Puts the text where the user wants it.
    fn deliver(&self, text: String) -> impl Future<Output = Result<(), String>> + Send + 'static;
    fn update(&self, update: Update);
}

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Phase(Phase),
    /// Microphone level, 0..1, about every 50 ms while recording.
    Level(f32),
    /// Live transcript.
    Text(String),
    Outcome(Outcome),
}

#[derive(Debug)]
enum Msg {
    Toggle,
    Event(Event),
}

/// Handle to the running controller. Cheap to clone.
#[derive(Clone)]
pub struct Controller {
    tx: mpsc::UnboundedSender<Msg>,
}

impl Controller {
    /// Returns the handle and the actor future; spawn the future on a tokio runtime.
    pub fn new<E: Env>(env: Arc<E>, timings: Timings) -> (Self, impl Future<Output = ()> + Send) {
        let (tx, rx) = mpsc::unbounded_channel();
        let actor = Actor {
            env,
            machine: Machine::new(timings),
            tx: tx.clone(),
            rx,
            session: None,
        };
        (Self { tx }, actor.run())
    }

    pub fn start(&self) {
        self.send(Msg::Event(Event::Start));
    }

    pub fn stop(&self) {
        self.send(Msg::Event(Event::Stop));
    }

    pub fn cancel(&self) {
        self.send(Msg::Event(Event::Cancel));
    }

    /// Starts when idle, stops when connecting or recording.
    pub fn toggle(&self) {
        self.send(Msg::Toggle);
    }

    fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_err() {
            tracing::error!("session controller is gone");
        }
    }
}

struct Session {
    sid: Sid,
    pipe: mpsc::UnboundedSender<PipeCmd>,
    pipe_task: AbortHandle,
    timers: Vec<AbortHandle>,
}

struct Actor<E> {
    env: Arc<E>,
    machine: Machine,
    tx: mpsc::UnboundedSender<Msg>,
    rx: mpsc::UnboundedReceiver<Msg>,
    session: Option<Session>,
}

impl<E: Env> Actor<E> {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            let event = match msg {
                Msg::Toggle => match self.machine.phase() {
                    Phase::Idle => Event::Start,
                    Phase::Connecting | Phase::Recording => Event::Stop,
                    Phase::Finalizing | Phase::Delivering => continue,
                },
                Msg::Event(e) => e,
            };
            if event == Event::Start
                && self.machine.phase() == Phase::Idle
                && self.env.connect_options().is_none()
            {
                tracing::info!("dictation requested without an API key");
                self.env.update(Update::Outcome(Outcome::NoKey));
                continue;
            }
            let before = self.machine.phase();
            let effects = self.machine.step(event);
            for effect in effects {
                self.apply(effect);
            }
            let after = self.machine.phase();
            if after != before {
                tracing::debug!(?before, ?after, "session phase");
                self.env.update(Update::Phase(after));
            }
        }
    }

    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::StartAudio { sid } => {
                let (pipe_tx, pipe_rx) = mpsc::unbounded_channel();
                let pipe = Pipe {
                    sid,
                    audio: self.env.start_audio(),
                    cmds: pipe_rx,
                    events: self.tx.clone(),
                    env: self.env.clone(),
                };
                let task = tokio::spawn(pipe.run());
                self.session = Some(Session {
                    sid,
                    pipe: pipe_tx,
                    pipe_task: task.abort_handle(),
                    timers: Vec::new(),
                });
            }
            Effect::StopAudio => self.pipe(PipeCmd::StopAudio),
            Effect::Connect { sid, attempt } => {
                tracing::info!(sid, attempt, "connecting");
                match self.env.connect_options() {
                    Some(options) => {
                        let timeout = self.machine.timings().connect_attempt;
                        self.pipe(PipeCmd::Connect(Box::new(options), timeout));
                    }
                    None => self.event_later(Event::ConnectFailed {
                        sid,
                        reason: "no API key".into(),
                    }),
                }
            }
            Effect::Finish { .. } => self.pipe(PipeCmd::Finish),
            Effect::Disconnect => {
                if let Some(session) = self.session.take() {
                    for timer in session.timers {
                        timer.abort();
                    }
                    // Dropping the command sender tells the pipe to close up.
                    drop(session.pipe);
                    let task = session.pipe_task;
                    tokio::spawn(async move {
                        tokio::time::sleep(CLOSE_GRACE).await;
                        task.abort();
                    });
                }
            }
            Effect::Arm { sid, timer, after } => {
                let tx = self.tx.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(after).await;
                    let _ = tx.send(Msg::Event(Event::Timeout { sid, timer }));
                });
                if let Some(s) = self.session.as_mut().filter(|s| s.sid == sid) {
                    s.timers.push(handle.abort_handle());
                }
            }
            Effect::Text(text) => self.env.update(Update::Text(text)),
            Effect::Deliver { sid, text } => {
                tracing::info!(sid, chars = text.chars().count(), "delivering");
                let delivery = self.env.deliver(text);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = delivery.await;
                    let _ = tx.send(Msg::Event(Event::Delivered { sid, result }));
                });
            }
            Effect::Outcome(outcome) => {
                tracing::info!(?outcome, "session ended");
                self.env.update(Update::Outcome(outcome));
            }
        }
    }

    fn pipe(&self, cmd: PipeCmd) {
        if let Some(s) = &self.session {
            let _ = s.pipe.send(cmd);
        }
    }

    /// Feeds an event back in after the current step.
    fn event_later(&self, event: Event) {
        let _ = self.tx.send(Msg::Event(event));
    }
}

enum PipeCmd {
    Connect(Box<qwen::ConnectOptions>, Duration),
    StopAudio,
    /// Send the finish frame once the microphone is done.
    Finish,
}

type Connecting =
    Pin<Box<dyn Future<Output = Result<(QwenSink, QwenStream, Handshake), ConnectError>> + Send>>;

/// Moves audio from the microphone to the server and server messages back
/// to the actor, for one session.
struct Pipe<E> {
    sid: Sid,
    audio: AudioInput,
    cmds: mpsc::UnboundedReceiver<PipeCmd>,
    events: mpsc::UnboundedSender<Msg>,
    env: Arc<E>,
}

impl<E: Env> Pipe<E> {
    async fn run(mut self) {
        let sid = self.sid;
        // Audio captured before the connection is up.
        let mut pending: Vec<Vec<u8>> = Vec::new();
        let mut connecting: Option<Connecting> = None;
        let mut sink: Option<QwenSink> = None;
        let mut stream: Option<QwenStream> = None;
        let mut audio_done = false;
        let mut finish_requested = false;
        let mut finish_sent = false;
        let mut send_failed = false;
        let mut sent_bytes = 0usize;

        loop {
            tokio::select! {
                cmd = self.cmds.recv() => match cmd {
                    Some(PipeCmd::Connect(options, timeout)) => {
                        connecting = Some(Box::pin(async move {
                            qwen::connect(&qwen::ConnectOptions { timeout, ..*options }).await
                        }));
                    }
                    Some(PipeCmd::StopAudio) => self.audio.stop(),
                    Some(PipeCmd::Finish) => finish_requested = true,
                    None => break,
                },
                result = async { connecting.as_mut().unwrap().await }, if connecting.is_some() => {
                    connecting = None;
                    match result {
                        Ok((mut s, st, handshake)) => {
                            tracing::info!(
                                sid,
                                ms = handshake.elapsed.as_millis(),
                                buffered_ms = pending.len() * crate::audio::dsp::CHUNK_MS,
                                "connected"
                            );
                            for pcm in pending.drain(..) {
                                sent_bytes += pcm.len();
                                if let Err(e) = s.audio(pcm).await {
                                    tracing::debug!(sid, "sending buffered audio: {e}");
                                    send_failed = true;
                                    break;
                                }
                            }
                            sink = Some(s);
                            stream = Some(st);
                            self.emit(Event::Connected { sid });
                        }
                        Err(ConnectError::KeyRejected) => self.emit(Event::KeyRejected { sid }),
                        Err(e) => self.emit(Event::ConnectFailed { sid, reason: e.to_string() }),
                    }
                },
                event = self.audio.events.recv(), if !audio_done => match event {
                    Some(AudioEvent::Started(device)) => tracing::info!(sid, "microphone: {device}"),
                    Some(AudioEvent::Chunk(chunk)) => {
                        self.env.update(Update::Level(chunk.level));
                        match sink.as_mut() {
                            Some(_) if send_failed => {}
                            Some(s) => {
                                sent_bytes += chunk.pcm.len();
                                if let Err(e) = s.audio(chunk.pcm).await {
                                    // The stream will say why; keep reading it.
                                    tracing::debug!(sid, "sending audio: {e}");
                                    send_failed = true;
                                }
                            }
                            None => pending.push(chunk.pcm),
                        }
                    }
                    Some(AudioEvent::Failed(reason)) => {
                        tracing::error!(sid, "microphone failed: {reason}");
                        self.emit(Event::AudioFailed { sid, reason });
                    }
                    None => audio_done = true,
                },
                event = async { stream.as_mut().unwrap().next().await }, if stream.is_some() => {
                    match event {
                        Some(event) => {
                            if let Some(e) = map_server_event(sid, event) {
                                self.emit(e);
                            }
                        }
                        None => stream = None,
                    }
                },
            }

            if finish_requested
                && audio_done
                && !finish_sent
                && !send_failed
                && let Some(s) = sink.as_mut()
            {
                finish_sent = true;
                tracing::debug!(sid, sent_bytes, "audio done; sending finish");
                if let Err(e) = s.finish().await {
                    tracing::debug!(sid, "sending finish: {e}");
                }
            }
        }

        // The actor let go of this session: stop everything.
        self.audio.stop();
        if let Some(mut s) = sink {
            let _ = tokio::time::timeout(CLOSE_GRACE, s.close()).await;
        }
        tracing::debug!(sid, sent_bytes, "pipe closed");
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(Msg::Event(event));
    }
}

fn map_server_event(sid: Sid, event: AsrEvent) -> Option<Event> {
    match event {
        AsrEvent::Server(ServerMsg::Result { text }) => Some(Event::Text { sid, text }),
        AsrEvent::Server(ServerMsg::Finish) => Some(Event::ServerFinished { sid }),
        AsrEvent::Server(ServerMsg::Error { code, message }) => Some(Event::ConnectionLost {
            sid,
            reason: format!("server error {code}: {message}"),
        }),
        AsrEvent::Server(ServerMsg::Unknown { event }) => {
            tracing::debug!(sid, "ignoring server event {event:?}");
            None
        }
        AsrEvent::Garbage(text) => {
            tracing::debug!(sid, "ignoring unparsable frame: {text}");
            None
        }
        AsrEvent::Closed { code, reason } => Some(Event::ConnectionLost {
            sid,
            reason: format!("closed ({code:?}, {reason:?})"),
        }),
        AsrEvent::Failed(reason) => Some(Event::ConnectionLost { sid, reason }),
    }
}
