//! End-to-end sessions against a scripted local WebSocket server, with fake
//! audio. Real time with short timings, so each test takes well under a second.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use moli_core::asr::Backend;
use moli_core::audio::{AudioEvent, AudioInput, Chunk};
use moli_core::doubao::web::ConnectOptions;
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
        finalize_cap: Duration::from_secs(3),
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
    backend: Option<Backend>,
    audio_fails: bool,
    produced: Arc<AtomicUsize>,
    rejected: AtomicBool,
    delivered: Mutex<Vec<String>>,
    updates: mpsc::UnboundedSender<Update>,
}

impl Env for TestEnv {
    fn backend(&self) -> Option<Backend> {
        self.backend.clone()
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
    let backend = addr.map(|addr| {
        Backend::Web(ConnectOptions {
            url: format!("ws://{addr}/samantha/audio/asr").parse().unwrap(),
            cookie_header: "sessionid=test".into(),
            origin: None,
            user_agent: None,
            timeout: Duration::from_secs(1),
        })
    });
    harness_for(backend, audio_fails)
}

fn harness_for(backend: Option<Backend>, audio_fails: bool) -> Harness {
    let (tx, updates) = mpsc::unbounded_channel();
    let env = Arc::new(TestEnv {
        backend,
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

// ---- the IME backend ----

mod ime {
    use super::*;
    use moli_core::doubao::ime::wire::{Request, Response, event};
    use moli_core::doubao::ime::{Endpoints, ImeClient};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ImeBehave {
        /// "你好" after some audio; "你好世界。" and SessionFinished after FinishSession.
        Normal,
        /// Answers StartTask with TaskFailed.
        FailStart,
        /// Never answers FinishSession.
        NoFinish,
        /// Answers the first audio with SessionFailed.
        FailMid,
        /// Like FailMid, with the error the service gives device ids it cannot route.
        Unroutable,
        /// Like Normal, but the speech comes in two segments, as after a pause.
        Segments,
        /// Sends a finished segment and the start of another, then drops the connection.
        Drop,
        /// Hears the rest of the speech after a Drop: "世界。" on finish.
        Rest,
        /// Like Normal, but after FinishSession the rest of the text trickles
        /// in for longer than the finalize timeout, as on a slow network.
        Slow,
    }

    #[derive(Default)]
    struct ImeState {
        ws_connections: AtomicUsize,
        audio_bytes: AtomicUsize,
        credential_fetches: AtomicUsize,
        /// Audio of the session that finished.
        finished_bytes: AtomicUsize,
    }

    struct ImeMock {
        client: ImeClient,
        state: Arc<ImeState>,
    }

    /// A WebSocket server for recognition and an HTTP server for credentials
    /// and organizing. WebSocket connection `n` gets `script[n]`.
    async fn ime_mock(script: Vec<ImeBehave>) -> ImeMock {
        let state = Arc::new(ImeState::default());

        let ws = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws.local_addr().unwrap();
        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = ws.accept().await {
                let n = st.ws_connections.fetch_add(1, Ordering::SeqCst);
                let behave = script[n.min(script.len() - 1)];
                tokio::spawn(serve_ime(tcp, behave, st.clone()));
            }
        });

        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_addr = http.local_addr().unwrap();
        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = http.accept().await {
                tokio::spawn(serve_http(tcp, st.clone()));
            }
        });

        let base = format!("http://{http_addr}");
        let endpoints = Endpoints {
            settings: format!("{base}/service/settings/v3/"),
            sami_config: format!("{base}/api/v1/user/get_config"),
            asr_ws: format!("ws://{ws_addr}/ocean/api/v1/ws"),
            organize: format!("{base}/api/v2/ai/text_organization"),
            ..Endpoints::default()
        };
        ImeMock {
            client: ImeClient::with_endpoints("1234567890123456", endpoints),
            state,
        }
    }

    fn frame(event: &str) -> Message {
        Message::binary(
            Response {
                event: event.into(),
                ..Default::default()
            }
            .to_bytes(),
        )
    }

    fn failure(event: &str) -> Message {
        failure_with(event, 40_000_012, "bad")
    }

    fn failure_with(event: &str, code: i64, text: &str) -> Message {
        Message::binary(
            Response {
                event: event.into(),
                status_code: code,
                status_text: text.into(),
                ..Default::default()
            }
            .to_bytes(),
        )
    }

    fn timed_segment(index: i64, start: f64, text: &str) -> Message {
        let result = serde_json::json!({"text": text, "index": index, "start_time": start});
        Message::binary(
            Response {
                payload: serde_json::json!({"results": [result]}).to_string(),
                ..Default::default()
            }
            .to_bytes(),
        )
    }

    fn ime_result(text: &str) -> Message {
        ime_segment(None, text)
    }

    fn ime_segment(index: Option<i64>, text: &str) -> Message {
        let mut result = serde_json::json!({"text": text});
        if let Some(i) = index {
            result["index"] = i.into();
        }
        Message::binary(
            Response {
                payload: serde_json::json!({"results": [result]}).to_string(),
                ..Default::default()
            }
            .to_bytes(),
        )
    }

    async fn serve_ime(tcp: TcpStream, behave: ImeBehave, state: Arc<ImeState>) {
        let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
            return;
        };
        let mut bytes = 0;
        let mut sent_partial = false;
        while let Some(Ok(msg)) = ws.next().await {
            let Message::Binary(data) = msg else { continue };
            let req = Request::from_bytes(&data).unwrap();
            let reply = match req.event.as_str() {
                event::START_TASK if behave == ImeBehave::FailStart => {
                    Some(failure(event::TASK_FAILED))
                }
                event::START_TASK => Some(frame(event::TASK_STARTED)),
                event::START_SESSION => {
                    assert_eq!(req.token, "TOK");
                    bytes = 0;
                    sent_partial = false;
                    Some(frame(event::SESSION_STARTED))
                }
                event::TASK_REQUEST => {
                    bytes += req.audio_data.len();
                    state
                        .audio_bytes
                        .fetch_add(req.audio_data.len(), Ordering::SeqCst);
                    if behave == ImeBehave::FailMid {
                        Some(failure(event::SESSION_FAILED))
                    } else if behave == ImeBehave::Unroutable {
                        Some(failure_with(
                            event::SESSION_FAILED,
                            50_700_000,
                            "read backend response: service discovery failure",
                        ))
                    } else if !sent_partial && bytes >= 2 * CHUNK_BYTES {
                        sent_partial = true;
                        if behave == ImeBehave::Drop {
                            let _ = ws.send(timed_segment(0, 0.0, "你好。")).await;
                            let _ = ws.send(timed_segment(1, 0.1, "世")).await;
                            return;
                        } else if behave == ImeBehave::Rest {
                            None
                        } else if behave == ImeBehave::Segments {
                            Some(ime_segment(Some(0), "你好。"))
                        } else {
                            Some(ime_result("你好"))
                        }
                    } else {
                        None
                    }
                }
                event::FINISH_SESSION if behave == ImeBehave::NoFinish => None,
                event::FINISH_SESSION => {
                    state.finished_bytes.store(bytes, Ordering::SeqCst);
                    if behave == ImeBehave::Slow {
                        for text in ["你好世", "你好世界"] {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            let _ = ws.send(ime_result(text)).await;
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    let last = if behave == ImeBehave::Rest {
                        timed_segment(0, 0.0, "世界。")
                    } else if behave == ImeBehave::Segments {
                        ime_segment(Some(1), "世界。")
                    } else {
                        ime_result("你好世界。")
                    };
                    let _ = ws.send(last).await;
                    Some(frame(event::SESSION_FINISHED))
                }
                _ => None,
            };
            if let Some(reply) = reply
                && ws.send(reply).await.is_err()
            {
                return;
            }
        }
    }

    /// Just enough HTTP/1.1 for reqwest: one request per connection.
    async fn serve_http(mut tcp: TcpStream, state: Arc<ImeState>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let (head_len, body_len) = loop {
            let Ok(n) = tcp.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                break (end + 4, len);
            }
        };
        while buf.len() < head_len + body_len {
            let Ok(n) = tcp.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let head = String::from_utf8_lossy(&buf[..head_len]);
        let path = head.split_whitespace().nth(1).unwrap_or("");
        let body = if path.starts_with("/service/settings/v3/") {
            state.credential_fetches.fetch_add(1, Ordering::SeqCst);
            r#"{"data":{"settings":{"asr_config":{"app_key":"AK"}}}}"#
        } else if path.starts_with("/api/v1/user/get_config") {
            r#"{"Data":{"sami_token":"TOK"}}"#
        } else if path.starts_with("/api/v2/ai/text_organization") {
            r#"{"code":0,"data":{"content":"明天几点开会？","no_rewrite":false}}"#
        } else {
            ""
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tcp.write_all(response.as_bytes()).await;
    }

    fn harness_ime(client: &ImeClient) -> Harness {
        harness_for(Some(Backend::Ime(client.clone())), false)
    }

    #[tokio::test]
    async fn dictates_and_sends_every_byte() {
        let mock = ime_mock(vec![ImeBehave::Normal]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好".into())).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好世界。"]);
        assert_eq!(mock.state.audio_bytes.load(Ordering::SeqCst), h.produced());
        assert_eq!(mock.state.credential_fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn credentials_are_reused_across_sessions() {
        let mock = ime_mock(vec![ImeBehave::Normal]).await;
        mock.client.prefetch().await;
        let mut h = harness_ime(&mock.client);
        for _ in 0..2 {
            h.ctl.start();
            h.recording().await;
            h.ctl.stop();
            assert_eq!(h.outcome().await, Outcome::Done { partial: false });
            h.wait_for(|u| *u == Update::Phase(Phase::Idle)).await;
        }
        assert_eq!(mock.state.credential_fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refused_handshake_refetches_credentials_and_retries() {
        let mock = ime_mock(vec![ImeBehave::FailStart, ImeBehave::Normal]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 2);
        assert_eq!(mock.state.credential_fetches.load(Ordering::SeqCst), 2);
        assert_eq!(mock.state.audio_bytes.load(Ordering::SeqCst), h.produced());
    }

    #[tokio::test]
    async fn no_finish_delivers_the_last_result() {
        let mock = ime_mock(vec![ImeBehave::NoFinish]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好".into())).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好"]);
    }

    #[tokio::test]
    async fn text_still_coming_after_the_stop_is_waited_for() {
        let mock = ime_mock(vec![ImeBehave::Slow]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好".into())).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好世界。"]);
    }

    #[tokio::test]
    async fn failure_before_any_text_is_a_network_error() {
        let mock = ime_mock(vec![ImeBehave::FailMid]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        assert!(matches!(h.outcome().await, Outcome::Network(_)));
        assert!(h.delivered().is_empty());
        // The first session and three replays.
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 4);
        assert!(!h.env.rejected.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unroutable_device_id_is_replaced_and_the_audio_replayed() {
        let mock = ime_mock(vec![ImeBehave::Unroutable, ImeBehave::Normal]).await;
        let saved = Arc::new(std::sync::Mutex::new(None));
        let s = saved.clone();
        mock.client
            .on_new_device_id(move |id| *s.lock().unwrap() = Some(id.to_string()));
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好".into())).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好世界。"]);
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 2);
        // The second session got everything, the audio sent to the first one included.
        assert_eq!(
            mock.state.finished_bytes.load(Ordering::SeqCst),
            h.produced()
        );
        let id = mock.client.device_id();
        assert_ne!(id, "1234567890123456");
        assert_eq!(id.len(), 16);
        assert_eq!(saved.lock().unwrap().as_deref(), Some(id.as_str()));
        assert_eq!(mock.state.credential_fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn segments_after_a_pause_are_all_delivered() {
        let mock = ime_mock(vec![ImeBehave::Segments]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好。".into())).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好。世界。"]);
    }

    #[tokio::test]
    async fn a_dropped_connection_carries_on_where_it_broke_off() {
        let mock = ime_mock(vec![ImeBehave::Drop, ImeBehave::Rest]).await;
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.wait_for(|u| *u == Update::Text("你好。世".into())).await;
        // Let the new session get going before the end.
        tokio::time::sleep(Duration::from_millis(300)).await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好。世界。"]);
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 2);
    }

    /// Turns on keeping a connection open and waits for one.
    async fn warmed(client: &ImeClient) {
        client.set_keep_warm(true);
        tokio::spawn(client.clone().warm());
        tokio::time::timeout(WAIT, async {
            while !client.is_warm() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("no warm connection");
    }

    #[tokio::test]
    async fn sessions_reuse_the_kept_open_connection() {
        let mock = ime_mock(vec![ImeBehave::Normal]).await;
        warmed(&mock.client).await;
        let mut h = harness_ime(&mock.client);
        for _ in 0..2 {
            h.ctl.start();
            h.recording().await;
            h.ctl.stop();
            assert_eq!(h.outcome().await, Outcome::Done { partial: false });
            h.wait_for(|u| *u == Update::Phase(Phase::Idle)).await;
            assert!(mock.client.is_warm());
        }
        assert_eq!(h.delivered(), ["你好世界。", "你好世界。"]);
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 1);
        assert_eq!(mock.state.credential_fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_up_replaces_an_unroutable_device_id() {
        let mock = ime_mock(vec![ImeBehave::Unroutable, ImeBehave::Normal]).await;
        let saved = Arc::new(std::sync::Mutex::new(None));
        let s = saved.clone();
        mock.client
            .on_new_device_id(move |id| *s.lock().unwrap() = Some(id.to_string()));
        warmed(&mock.client).await;
        let id = mock.client.device_id();
        assert_ne!(id, "1234567890123456");
        assert_eq!(saved.lock().unwrap().as_deref(), Some(id.as_str()));

        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert_eq!(h.delivered(), ["你好世界。"]);
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn turning_warm_off_closes_the_connection() {
        let mock = ime_mock(vec![ImeBehave::Normal]).await;
        warmed(&mock.client).await;
        mock.client.set_keep_warm(false);
        assert!(!mock.client.is_warm());
        let mut h = harness_ime(&mock.client);
        h.ctl.start();
        h.recording().await;
        h.ctl.stop();
        assert_eq!(h.outcome().await, Outcome::Done { partial: false });
        assert!(!mock.client.is_warm());
        assert_eq!(mock.state.ws_connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn organize_rewrites_the_text() {
        let mock = ime_mock(vec![ImeBehave::Normal]).await;
        let out = mock
            .client
            .organize("嗯那个明天几点开会", Duration::from_secs(2))
            .await;
        assert_eq!(out.as_deref(), Some("明天几点开会？"));
    }

    #[tokio::test]
    async fn organize_gives_up_on_an_unreachable_service() {
        // Bind and drop, so nothing listens on the port.
        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let endpoints = Endpoints {
            organize: format!("http://127.0.0.1:{port}/api/v2/ai/text_organization"),
            ..Endpoints::default()
        };
        let client = ImeClient::with_endpoints(ImeClient::new_device_id(), endpoints);
        let out = client.organize("嗯那个", Duration::from_secs(2)).await;
        assert_eq!(out, None);
    }
}
