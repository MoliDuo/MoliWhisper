//! End-to-end sessions against a scripted local WebSocket server, with fake
//! audio. Real time with short timings, so each test takes well under a second.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use moli_core::asr::ConnectOptions;
use moli_core::audio::{AudioEvent, AudioInput, Chunk};
use moli_core::session::{Controller, Env, Outcome, Phase, Timings, Update};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const CHUNK_BYTES: usize = 1600; // 50 ms of 16 kHz s16le
const WAIT: Duration = Duration::from_secs(5);

fn timings() -> Timings {
    Timings {
        max_attempts: 3,
        connect_attempt: Duration::from_millis(400),
        connect_deadline: Duration::from_secs(3),
        finalize: Duration::from_millis(400),
        max_duration: Duration::from_secs(10),
    }
}

// ---- mock server ----

#[derive(Debug, Clone, Copy, PartialEq)]
enum Behave {
    /// "你好" after some audio; "你好世界。" and `finish` after our finish.
    Normal,
    /// Like `Normal`, but the handshake waits this long first.
    SlowHandshake(u64),
    /// Never answers our finish.
    NoFinish,
    /// Recognizes nothing.
    Silent,
    /// Error code 710022013 on the first audio.
    RejectCode,
    /// Closes with reason "2013" on the first audio.
    Close2013,
    Http403,
    /// Accepts TCP, never completes the handshake.
    Stall,
    /// Sends "你好", then drops the connection without a close frame.
    DropMid,
}

#[derive(Default)]
struct ServerState {
    connections: AtomicUsize,
    audio_bytes: AtomicUsize,
    finish_seen: AtomicBool,
}

struct Mock {
    addr: SocketAddr,
    state: Arc<ServerState>,
}

/// Connection `n` gets `script[n]`; the last entry repeats.
async fn mock(script: Vec<Behave>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(ServerState::default());
    let st = state.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let n = st.connections.fetch_add(1, Ordering::SeqCst);
            let behave = script[n.min(script.len() - 1)];
            tokio::spawn(serve(tcp, behave, st.clone()));
        }
    });
    Mock { addr, state }
}

fn result(text: &str) -> Message {
    Message::text(
        serde_json::json!({"event":"result","result":{"Text":text},"code":0,"message":""})
            .to_string(),
    )
}

fn finish() -> Message {
    Message::text(r#"{"event":"finish","result":null,"code":0,"message":""}"#)
}

async fn serve(tcp: TcpStream, behave: Behave, state: Arc<ServerState>) {
    match behave {
        Behave::Stall => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        Behave::Http403 => {
            #[allow(clippy::result_large_err)] // the signature tungstenite asks for
            let reject = |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
                Err(http::Response::builder().status(403).body(None).unwrap())
            };
            let _ = tokio_tungstenite::accept_hdr_async(tcp, reject).await;
            return;
        }
        Behave::SlowHandshake(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
        _ => {}
    }
    let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
        return;
    };
    let mut bytes = 0;
    let mut sent_partial = false;
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Binary(b) => {
                bytes += b.len();
                state.audio_bytes.fetch_add(b.len(), Ordering::SeqCst);
                match behave {
                    Behave::RejectCode => {
                        let _ = ws
                            .send(Message::text(
                                r#"{"code":710022013,"message":"tourist reach limited"}"#,
                            ))
                            .await;
                        let _ = ws.close(None).await;
                        return;
                    }
                    Behave::Close2013 => {
                        let frame = CloseFrame {
                            code: CloseCode::Normal,
                            reason: "2013".into(),
                        };
                        let _ = ws.close(Some(frame)).await;
                        return;
                    }
                    Behave::Silent => {}
                    _ if !sent_partial && bytes >= 2 * CHUNK_BYTES => {
                        sent_partial = true;
                        let _ = ws.send(result("你好")).await;
                        if behave == Behave::DropMid {
                            return; // drops the socket, no close frame
                        }
                    }
                    _ => {}
                }
            }
            Message::Text(t) if t.contains(r#""finish""#) => {
                state.finish_seen.store(true, Ordering::SeqCst);
                match behave {
                    Behave::NoFinish => {}
                    Behave::Silent => {
                        let _ = ws.send(result("")).await;
                        let _ = ws.send(finish()).await;
                    }
                    _ => {
                        let _ = ws.send(result("你好世界。")).await;
                        let _ = ws.send(finish()).await;
                    }
                }
            }
            Message::Close(_) => return,
            _ => {}
        }
    }
}

// ---- fake app ----

struct TestEnv {
    addr: Option<SocketAddr>,
    audio_fails: bool,
    produced: Arc<AtomicUsize>,
    rejected: AtomicBool,
    delivered: Mutex<Vec<String>>,
    updates: mpsc::UnboundedSender<Update>,
}

impl Env for TestEnv {
    fn connect_options(&self) -> Option<ConnectOptions> {
        let addr = self.addr?;
        Some(ConnectOptions {
            url: format!("ws://{addr}/samantha/audio/asr").parse().unwrap(),
            cookie_header: "sessionid=test".into(),
            origin: None,
            user_agent: None,
            timeout: Duration::from_secs(1),
        })
    }

    fn start_audio(&self) -> AudioInput {
        fake_audio(self.produced.clone(), self.audio_fails)
    }

    fn deliver(&self, text: String) -> impl Future<Output = Result<(), String>> + Send + 'static {
        self.delivered.lock().unwrap().push(text);
        async { Ok(()) }
    }

    fn session_rejected(&self) {
        self.rejected.store(true, Ordering::SeqCst);
    }

    fn update(&self, update: Update) {
        let _ = self.updates.send(update);
    }
}

/// A chunk every 10 ms until stopped; counts the bytes it produced.
fn fake_audio(produced: Arc<AtomicUsize>, fails: bool) -> AudioInput {
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    tokio::spawn(async move {
        let _ = tx.send(AudioEvent::Started("fake".into()));
        if fails {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(AudioEvent::Failed("unplugged".into()));
            return;
        }
        while !stopped.load(Ordering::SeqCst) {
            produced.fetch_add(CHUNK_BYTES, Ordering::SeqCst);
            let chunk = Chunk {
                pcm: vec![0; CHUNK_BYTES],
                level: 0.5,
            };
            if tx.send(AudioEvent::Chunk(chunk)).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    AudioInput::new(rx, move || stop.store(true, Ordering::SeqCst))
}

struct Harness {
    ctl: Controller,
    env: Arc<TestEnv>,
    updates: mpsc::UnboundedReceiver<Update>,
    /// Everything but levels, in order.
    seen: Vec<Update>,
}

fn harness(addr: Option<SocketAddr>) -> Harness {
    harness_with(addr, false)
}

fn harness_with(addr: Option<SocketAddr>, audio_fails: bool) -> Harness {
    let (tx, updates) = mpsc::unbounded_channel();
    let env = Arc::new(TestEnv {
        addr,
        audio_fails,
        produced: Arc::default(),
        rejected: AtomicBool::new(false),
        delivered: Mutex::default(),
        updates: tx,
    });
    let (ctl, actor) = Controller::new(env.clone(), timings());
    tokio::spawn(actor);
    Harness {
        ctl,
        env,
        updates,
        seen: Vec::new(),
    }
}

impl Harness {
    async fn wait_for(&mut self, want: impl Fn(&Update) -> bool) -> Update {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let update = tokio::time::timeout_at(deadline, self.updates.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out; saw {:?}", self.seen))
                .expect("controller gone");
            if !matches!(update, Update::Level(_)) {
                self.seen.push(update.clone());
            }
            if want(&update) {
                return update;
            }
        }
    }

    async fn recording(&mut self) {
        self.wait_for(|u| *u == Update::Phase(Phase::Recording))
            .await;
    }

    async fn outcome(&mut self) -> Outcome {
        match self.wait_for(|u| matches!(u, Update::Outcome(_))).await {
            Update::Outcome(o) => o,
            _ => unreachable!(),
        }
    }

    fn delivered(&self) -> Vec<String> {
        self.env.delivered.lock().unwrap().clone()
    }

    fn produced(&self) -> usize {
        self.env.produced.load(Ordering::SeqCst)
    }
}

// ---- scenarios ----

#[tokio::test]
async fn dictates_and_sends_every_byte() {
    let server = mock(vec![Behave::Normal]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    h.recording().await;
    h.wait_for(|u| *u == Update::Text("你好".into())).await;
    h.ctl.stop();
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    assert_eq!(h.delivered(), ["你好世界。"]);
    assert_eq!(
        server.state.audio_bytes.load(Ordering::SeqCst),
        h.produced()
    );
    assert!(h.seen.contains(&Update::Phase(Phase::Finalizing)));
    h.wait_for(|u| *u == Update::Phase(Phase::Idle)).await;
    assert_eq!(server.state.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn audio_from_before_the_connection_is_not_lost() {
    let server = mock(vec![Behave::SlowHandshake(200)]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.ctl.stop(); // still connecting
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    assert!(h.produced() > 0);
    assert_eq!(
        server.state.audio_bytes.load(Ordering::SeqCst),
        h.produced()
    );
    assert!(!h.seen.contains(&Update::Phase(Phase::Recording)));
}

#[tokio::test]
async fn rejection_code_marks_the_login_once() {
    let server = mock(vec![Behave::RejectCode]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    assert_eq!(h.outcome().await, Outcome::SessionRejected);
    assert!(h.env.rejected.load(Ordering::SeqCst));
    assert_eq!(server.state.connections.load(Ordering::SeqCst), 1);
    assert!(h.delivered().is_empty());
}

#[tokio::test]
async fn close_reason_2013_is_a_rejection() {
    let server = mock(vec![Behave::Close2013]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    assert_eq!(h.outcome().await, Outcome::SessionRejected);
    assert!(h.env.rejected.load(Ordering::SeqCst));
}

#[tokio::test]
async fn http_403_is_retried_then_a_network_error() {
    let server = mock(vec![Behave::Http403]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    assert!(matches!(h.outcome().await, Outcome::Network(_)));
    assert_eq!(server.state.connections.load(Ordering::SeqCst), 3);
    assert!(
        !h.env.rejected.load(Ordering::SeqCst),
        "not an auth problem"
    );
}

#[tokio::test]
async fn retry_recovers_and_keeps_the_audio() {
    let server = mock(vec![Behave::Http403, Behave::Normal]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    h.recording().await;
    h.ctl.stop();
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    assert_eq!(server.state.connections.load(Ordering::SeqCst), 2);
    assert_eq!(
        server.state.audio_bytes.load(Ordering::SeqCst),
        h.produced()
    );
}

#[tokio::test]
async fn stalled_handshakes_time_out() {
    let server = mock(vec![Behave::Stall]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    assert!(matches!(h.outcome().await, Outcome::Network(_)));
    assert_eq!(server.state.connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn missing_finish_falls_back_to_the_last_result() {
    let server = mock(vec![Behave::NoFinish]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    h.wait_for(|u| *u == Update::Text("你好".into())).await;
    h.ctl.stop();
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    assert!(server.state.finish_seen.load(Ordering::SeqCst));
    assert_eq!(h.delivered(), ["你好"]);
}

#[tokio::test]
async fn dropped_connection_delivers_what_it_has() {
    let server = mock(vec![Behave::DropMid]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    assert_eq!(h.outcome().await, Outcome::Done { partial: true });
    assert_eq!(h.delivered(), ["你好"]);
}

#[tokio::test]
async fn silence_delivers_nothing() {
    let server = mock(vec![Behave::Silent]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    h.recording().await;
    h.ctl.stop();
    assert_eq!(h.outcome().await, Outcome::Empty);
    assert!(h.delivered().is_empty());
}

#[tokio::test]
async fn cancel_delivers_nothing() {
    let server = mock(vec![Behave::Normal]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.start();
    h.wait_for(|u| *u == Update::Text("你好".into())).await;
    h.ctl.cancel();
    assert_eq!(h.outcome().await, Outcome::Cancelled);
    assert!(h.delivered().is_empty());
    assert!(!server.state.finish_seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn toggle_starts_and_stops() {
    let server = mock(vec![Behave::Normal]).await;
    let mut h = harness(Some(server.addr));
    h.ctl.toggle();
    h.recording().await;
    h.ctl.toggle();
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    // And again, on a fresh session.
    h.ctl.toggle();
    h.recording().await;
    h.ctl.toggle();
    assert_eq!(h.outcome().await, Outcome::Done { partial: false });
    assert_eq!(h.delivered().len(), 2);
}

#[tokio::test]
async fn needs_a_login_first() {
    let mut h = harness(None);
    h.ctl.start();
    assert_eq!(h.outcome().await, Outcome::NeedLogin);
    assert!(h.seen.iter().all(|u| !matches!(u, Update::Phase(_))));
}

#[tokio::test]
async fn microphone_failure_ends_the_session() {
    let server = mock(vec![Behave::Normal]).await;
    let mut h = harness_with(Some(server.addr), true);
    h.ctl.start();
    assert_eq!(h.outcome().await, Outcome::Microphone("unplugged".into()));
}
