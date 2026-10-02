//! The Qwen ASR client, the session controller and the organizer against
//! local mock servers.

#![allow(clippy::result_large_err)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use moli_core::asr::{AsrEvent, ConnectError, ServerMsg};
use moli_core::audio::{AudioEvent, AudioInput, Chunk};
use moli_core::organize::Organizer;
use moli_core::qwen::{self, ConnectOptions, QwenStream};
use moli_core::session::{Controller, Env, Outcome, Phase, Timings, Update};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;

const T: Duration = Duration::from_secs(2);

/// How the mock DashScope server behaves.
#[derive(Clone, Default)]
struct Behavior {
    /// Wait this long before answering the WebSocket upgrade.
    handshake_delay: Duration,
    /// Drop this many connections right after they open.
    drop_first: usize,
    /// Never answer `finish-task`.
    ignore_finish: bool,
    /// Close the connection after the second chunk of audio.
    hang_up_after_audio: bool,
    /// Recognize nothing.
    silent: bool,
}

#[derive(Default)]
struct Stats {
    connections: AtomicUsize,
    audio_bytes: AtomicUsize,
}

/// A DashScope-like WebSocket server wanting `Bearer key`: `run-task` is
/// answered with `task-started`, the first audio with an open sentence, the
/// second with a finished one, and `finish-task` with a last sentence and
/// `task-finished`. A `fail` model gets `task-failed` instead of `task-started`.
async fn ws_server(key: &'static str, behavior: Behavior) -> (SocketAddr, Arc<Stats>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stats = Arc::new(Stats::default());
    let server_stats = stats.clone();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let n = server_stats.connections.fetch_add(1, Ordering::SeqCst);
            let behavior = behavior.clone();
            let stats = server_stats.clone();
            tokio::spawn(async move {
                if n < behavior.drop_first {
                    return;
                }
                tokio::time::sleep(behavior.handshake_delay).await;
                let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok());
                    if auth == Some(&format!("Bearer {key}")) {
                        Ok(resp)
                    } else {
                        let mut r = ErrorResponse::new(None);
                        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
                        Err(r)
                    }
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, check).await else {
                    return;
                };
                let event = |name: &str, payload: &str| {
                    Message::Text(
                        format!(r#"{{"header":{{"event":"{name}"}},"payload":{payload}}}"#).into(),
                    )
                };
                let sentence = |text: &str, end: &str| {
                    event(
                        "result-generated",
                        &format!(
                            r#"{{"output":{{"sentence":{{"begin_time":0,"end_time":{end},"text":"{text}"}}}}}}"#
                        ),
                    )
                };
                let mut audio = 0;
                while let Some(Ok(msg)) = ws.next().await {
                    match msg {
                        Message::Text(t) if t.contains("run-task") => {
                            let reply = if t.contains(r#""model":"fail""#) {
                                Message::Text(
                                    r#"{"header":{"event":"task-failed","error_code":"Oops","error_message":"no"}}"#.into(),
                                )
                            } else {
                                event("task-started", "{}")
                            };
                            ws.send(reply).await.unwrap();
                        }
                        Message::Binary(pcm) => {
                            stats.audio_bytes.fetch_add(pcm.len(), Ordering::SeqCst);
                            audio += 1;
                            if behavior.silent {
                                continue;
                            }
                            let m = match audio {
                                1 => sentence("你好", "null"),
                                2 => sentence("你好。", "1800"),
                                _ => continue,
                            };
                            ws.send(m).await.unwrap();
                            if behavior.hang_up_after_audio && audio == 2 {
                                let _ = ws.close(None).await;
                                return;
                            }
                        }
                        Message::Text(t) if t.contains("finish-task") => {
                            if behavior.ignore_finish {
                                continue;
                            }
                            if !behavior.silent {
                                ws.send(sentence("世界。", "3000")).await.unwrap();
                            }
                            ws.send(event("task-finished", "{}")).await.unwrap();
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    (addr, stats)
}

fn opts(addr: SocketAddr, key: &str, model: &str) -> ConnectOptions {
    ConnectOptions {
        url: format!("ws://{addr}/ws").parse().unwrap(),
        api_key: key.into(),
        model: model.into(),
        timeout: T,
    }
}

async fn next(stream: &mut QwenStream) -> AsrEvent {
    tokio::time::timeout(T, stream.next())
        .await
        .unwrap()
        .unwrap()
}

fn result(text: &str) -> AsrEvent {
    AsrEvent::Server(ServerMsg::Result { text: text.into() })
}

#[tokio::test]
async fn sentences_accumulate_into_the_full_transcript() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    let (mut sink, mut stream, _) = qwen::connect(&opts(addr, "secret", "m")).await.unwrap();
    sink.audio(vec![0; 1600]).await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好"));
    sink.audio(vec![0; 1600]).await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好。"));
    sink.finish().await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好。世界。"));
    assert_eq!(next(&mut stream).await, AsrEvent::Server(ServerMsg::Finish));
}

#[tokio::test]
async fn wrong_key_is_rejected() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    match qwen::connect(&opts(addr, "nope", "m")).await {
        Err(ConnectError::KeyRejected) => {}
        other => panic!("unexpected: {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn task_failed_at_the_start_is_a_connect_error() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    match qwen::connect(&opts(addr, "secret", "fail")).await {
        Err(ConnectError::Transient(m)) => assert!(m.contains("Oops"), "{m}"),
        other => panic!("unexpected: {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn check_reports_the_outcome() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    assert!(qwen::check(&opts(addr, "secret", "m")).await.is_ok());
    let err = qwen::check(&opts(addr, "nope", "m")).await.unwrap_err();
    assert!(err.contains("API key"), "{err}");
}

#[test]
fn options_need_a_key_and_default_to_the_cloud_service() {
    assert!(ConnectOptions::new("  ").is_none());
    let o = ConnectOptions::new(" k ").unwrap();
    assert_eq!(o.url.as_str(), qwen::DEFAULT_URL);
    assert_eq!(o.model, qwen::DEFAULT_MODEL);
    assert_eq!(o.api_key, "k");
}

// ---- sessions ----------------------------------------------------------

/// Chunks of silence every 10 ms until stopped, or a microphone failure.
#[derive(Clone, Copy)]
enum Mic {
    Works,
    Broken,
}

struct TestEnv {
    options: Option<ConnectOptions>,
    mic: Mic,
    chunks: Arc<AtomicUsize>,
    delivered: Mutex<Vec<String>>,
    updates: mpsc::UnboundedSender<Update>,
}

impl Env for TestEnv {
    fn connect_options(&self) -> Option<ConnectOptions> {
        self.options.clone()
    }

    fn start_audio(&self) -> AudioInput {
        let (tx, rx) = mpsc::unbounded_channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let chunks = self.chunks.clone();
        let mic = self.mic;
        tokio::spawn(async move {
            if let Mic::Broken = mic {
                let _ = tx.send(AudioEvent::Failed("no microphone".into()));
                return;
            }
            let _ = tx.send(AudioEvent::Started("test".into()));
            while !flag.load(Ordering::SeqCst) {
                chunks.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(AudioEvent::Chunk(Chunk {
                    pcm: vec![0; 1600],
                    level: 0.1,
                }));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        AudioInput::new(rx, move || stopped.store(true, Ordering::SeqCst))
    }

    fn deliver(&self, text: String) -> impl Future<Output = Result<(), String>> + Send + 'static {
        self.delivered.lock().unwrap().push(text);
        async { Ok(()) }
    }

    fn update(&self, update: Update) {
        let _ = self.updates.send(update);
    }
}

fn short_timings() -> Timings {
    Timings {
        max_attempts: 3,
        connect_attempt: Duration::from_millis(400),
        connect_deadline: Duration::from_secs(3),
        finalize: Duration::from_millis(400),
        finalize_cap: Duration::from_secs(3),
        max_duration: Duration::from_secs(10),
    }
}

struct Rig {
    controller: Controller,
    env: Arc<TestEnv>,
    updates: mpsc::UnboundedReceiver<Update>,
    phases: Vec<Phase>,
}

fn rig(options: Option<ConnectOptions>, mic: Mic) -> Rig {
    let (tx, updates) = mpsc::unbounded_channel();
    let env = Arc::new(TestEnv {
        options,
        mic,
        chunks: Arc::new(AtomicUsize::new(0)),
        delivered: Mutex::new(Vec::new()),
        updates: tx,
    });
    let (controller, actor) = Controller::new(env.clone(), short_timings());
    tokio::spawn(actor);
    Rig {
        controller,
        env,
        updates,
        phases: Vec::new(),
    }
}

impl Rig {
    /// Waits for the next update that is not a level or live text.
    async fn next_update(&mut self) -> Update {
        loop {
            let u = tokio::time::timeout(Duration::from_secs(8), self.updates.recv())
                .await
                .expect("timed out waiting for the session")
                .expect("the controller stopped");
            match u {
                Update::Level(_) | Update::Text(_) => {}
                Update::Phase(p) => {
                    self.phases.push(p);
                    return Update::Phase(p);
                }
                other => return other,
            }
        }
    }

    async fn wait_phase(&mut self, phase: Phase) {
        loop {
            if self.next_update().await == Update::Phase(phase) {
                return;
            }
        }
    }

    async fn outcome(&mut self) -> Outcome {
        loop {
            if let Update::Outcome(o) = self.next_update().await {
                return o;
            }
        }
    }

    /// Starts, records for `ms`, stops, and returns how it ended.
    async fn dictate(&mut self, ms: u64) -> Outcome {
        self.controller.start();
        self.wait_phase(Phase::Recording).await;
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.controller.stop();
        self.outcome().await
    }

    fn delivered(&self) -> Vec<String> {
        self.env.delivered.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn a_session_dictates_and_sends_every_byte() {
    let (addr, stats) = ws_server("secret", Behavior::default()).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    assert_eq!(rig.dictate(300).await, Outcome::Done { partial: false });
    assert_eq!(rig.delivered(), vec!["你好。世界。"]);
    assert_eq!(
        stats.audio_bytes.load(Ordering::SeqCst),
        rig.env.chunks.load(Ordering::SeqCst) * 1600
    );
}

#[tokio::test]
async fn audio_from_before_the_connection_is_not_lost() {
    let behavior = Behavior {
        handshake_delay: Duration::from_millis(200),
        ..Behavior::default()
    };
    let (addr, stats) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    assert_eq!(rig.dictate(300).await, Outcome::Done { partial: false });
    assert_eq!(
        stats.audio_bytes.load(Ordering::SeqCst),
        rig.env.chunks.load(Ordering::SeqCst) * 1600
    );
}

#[tokio::test]
async fn a_second_attempt_recovers_from_a_dropped_connection() {
    let behavior = Behavior {
        drop_first: 1,
        ..Behavior::default()
    };
    let (addr, stats) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    assert_eq!(rig.dictate(300).await, Outcome::Done { partial: false });
    assert_eq!(stats.connections.load(Ordering::SeqCst), 2);
    assert_eq!(rig.delivered(), vec!["你好。世界。"]);
}

#[tokio::test]
async fn a_stalled_handshake_times_out_after_three_attempts() {
    let behavior = Behavior {
        handshake_delay: Duration::from_secs(30),
        ..Behavior::default()
    };
    let (addr, stats) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    rig.controller.start();
    assert!(matches!(rig.outcome().await, Outcome::Network(_)));
    assert_eq!(stats.connections.load(Ordering::SeqCst), 3);
    assert!(rig.delivered().is_empty());
}

#[tokio::test]
async fn a_missing_finish_falls_back_to_the_last_result() {
    let behavior = Behavior {
        ignore_finish: true,
        ..Behavior::default()
    };
    let (addr, _) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    assert_eq!(rig.dictate(300).await, Outcome::Done { partial: false });
    assert_eq!(rig.delivered(), vec!["你好。"]);
}

#[tokio::test]
async fn a_dropped_connection_delivers_what_there_is() {
    let behavior = Behavior {
        hang_up_after_audio: true,
        ..Behavior::default()
    };
    let (addr, _) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    rig.controller.start();
    assert_eq!(rig.outcome().await, Outcome::Done { partial: true });
    assert_eq!(rig.delivered(), vec!["你好。"]);
}

#[tokio::test]
async fn silence_gives_an_empty_outcome() {
    let behavior = Behavior {
        silent: true,
        ..Behavior::default()
    };
    let (addr, _) = ws_server("secret", behavior).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    assert_eq!(rig.dictate(200).await, Outcome::Empty);
    assert!(rig.delivered().is_empty());
}

#[tokio::test]
async fn cancel_delivers_nothing() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    rig.controller.start();
    rig.wait_phase(Phase::Recording).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.controller.cancel();
    assert_eq!(rig.outcome().await, Outcome::Cancelled);
    assert!(rig.delivered().is_empty());
}

#[tokio::test]
async fn toggle_starts_and_stops_twice() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Works);
    for _ in 0..2 {
        rig.controller.toggle();
        rig.wait_phase(Phase::Recording).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        rig.controller.toggle();
        assert_eq!(rig.outcome().await, Outcome::Done { partial: false });
    }
    assert_eq!(rig.delivered().len(), 2);
}

#[tokio::test]
async fn without_a_key_nothing_starts() {
    let mut rig = rig(None, Mic::Works);
    rig.controller.start();
    assert_eq!(rig.next_update().await, Update::Outcome(Outcome::NoKey));
    assert!(rig.phases.is_empty());
    assert_eq!(rig.env.chunks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_broken_microphone_ends_the_session() {
    let (addr, _) = ws_server("secret", Behavior::default()).await;
    let mut rig = rig(Some(opts(addr, "secret", "m")), Mic::Broken);
    rig.controller.start();
    assert!(matches!(rig.outcome().await, Outcome::Microphone(_)));
}

#[tokio::test]
async fn a_wrong_key_is_reported_without_retrying() {
    let (addr, stats) = ws_server("secret", Behavior::default()).await;
    let mut rig = rig(Some(opts(addr, "nope", "m")), Mic::Works);
    rig.controller.start();
    assert_eq!(rig.outcome().await, Outcome::KeyRejected);
    assert_eq!(stats.connections.load(Ordering::SeqCst), 1);
}

/// One-shot HTTP server answering every request with `status` and `body`
/// after `delay`; returns its base URL and a channel with the raw requests.
async fn http_server(
    status: u16,
    body: &'static str,
    delay: Duration,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // Read until the headers and the whole body are in.
                loop {
                    let n = tcp.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if rest.len() >= len {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
                tokio::time::sleep(delay).await;
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tcp.write_all(reply.as_bytes()).await;
            });
        }
    });
    (url, rx)
}

fn organizer(base_url: String) -> Organizer {
    Organizer::new(&base_url, "m", "sk-test", None).unwrap()
}

#[tokio::test]
async fn organizer_sends_the_request_and_cleans_the_reply() {
    let (url, mut requests) = http_server(
        200,
        r#"{"choices":[{"message":{"content":"<think>x</think>\n今天天气很好。 "}}]}"#,
        Duration::ZERO,
    )
    .await;
    let out = organizer(url).organize("嗯 今天天气 很好", T).await;
    assert_eq!(out.as_deref(), Some("今天天气很好。"));
    let req = requests.recv().await.unwrap();
    assert!(req.starts_with("POST /v1/chat/completions"));
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer sk-test")
    );
    assert!(req.contains("嗯 今天天气 很好"));
}

#[tokio::test]
async fn organizer_failures_give_none() {
    let (url, _r) = http_server(500, "{}", Duration::ZERO).await;
    assert_eq!(organizer(url).organize("a", T).await, None);

    let (url, _r) = http_server(
        200,
        r#"{"choices":[{"message":{"content":"  "}}]}"#,
        Duration::ZERO,
    )
    .await;
    assert_eq!(organizer(url).organize("a", T).await, None);

    let (url, _r) = http_server(
        200,
        r#"{"choices":[{"message":{"content":"late"}}]}"#,
        Duration::from_millis(500),
    )
    .await;
    assert_eq!(
        organizer(url)
            .organize("a", Duration::from_millis(100))
            .await,
        None
    );
}
