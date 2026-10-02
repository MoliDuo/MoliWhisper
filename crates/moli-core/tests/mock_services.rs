//! The Qwen ASR client and the OpenAI-compatible organizer against
//! local mock servers.

#![allow(clippy::result_large_err)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use moli_core::asr::{AsrEvent, Backend, ConnectError, ServerMsg};
use moli_core::config::OpenAiConfig;
use moli_core::organize::{OpenAiOrganizer, Organizer};
use moli_core::qwen::{self, ConnectOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;

const T: Duration = Duration::from_secs(2);

/// A DashScope-like WebSocket server wanting `Bearer key`: `run-task` is
/// answered with `task-started`, the first audio with an open sentence, the
/// second with a finished one, and `finish-task` with a last sentence and
/// `task-finished`. A `fail` model gets `task-failed` instead of `task-started`.
async fn ws_server(key: &'static str) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
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
                        Message::Binary(_) => {
                            audio += 1;
                            let m = if audio == 1 {
                                sentence("你好", "null")
                            } else {
                                sentence("你好。", "1800")
                            };
                            ws.send(m).await.unwrap();
                        }
                        Message::Text(t) if t.contains("finish-task") => {
                            ws.send(sentence("世界。", "3000")).await.unwrap();
                            ws.send(event("task-finished", "{}")).await.unwrap();
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    addr
}

fn opts(addr: std::net::SocketAddr, key: &str, model: &str) -> ConnectOptions {
    ConnectOptions::new(&format!("ws://{addr}/ws"), key, model).unwrap()
}

async fn next(stream: &mut moli_core::asr::Stream) -> AsrEvent {
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
    let addr = ws_server("secret").await;
    let backend = Backend::Qwen(opts(addr, "secret", ""));
    let (mut sink, mut stream, _) = backend.connect(T).await.unwrap();
    sink.audio(vec![0; 1600]).await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好"));
    sink.audio(vec![0; 1600]).await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好。"));
    sink.finish().await.unwrap();
    assert_eq!(next(&mut stream).await, result("你好。世界。"));
    assert_eq!(next(&mut stream).await, AsrEvent::Server(ServerMsg::Finish));
}

#[tokio::test]
async fn wrong_key_is_rejected_with_401() {
    let addr = ws_server("secret").await;
    let backend = Backend::Qwen(opts(addr, "nope", ""));
    match backend.connect(T).await {
        Err(ConnectError::Rejected(401)) => {}
        other => panic!("unexpected: {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn task_failed_at_the_start_is_a_connect_error() {
    let addr = ws_server("secret").await;
    let backend = Backend::Qwen(opts(addr, "secret", "fail"));
    match backend.connect(T).await {
        Err(ConnectError::Transient(m)) => assert!(m.contains("Oops"), "{m}"),
        other => panic!("unexpected: {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn check_reports_the_outcome() {
    let addr = ws_server("secret").await;
    assert!(qwen::check(&opts(addr, "secret", "")).await.is_ok());
    let err = qwen::check(&opts(addr, "nope", "")).await.unwrap_err();
    assert!(err.contains("API key"), "{err}");
}

#[test]
fn options_need_a_key_and_a_websocket_url() {
    assert!(ConnectOptions::new("", "", "").is_none());
    assert!(ConnectOptions::new("http://x", "k", "").is_none());
    assert!(ConnectOptions::new("not a url", "k", "").is_none());
    let o = ConnectOptions::new("", " k ", "").unwrap();
    assert_eq!(o.url.as_str(), qwen::DEFAULT_URL);
    assert_eq!(o.model, qwen::DEFAULT_MODEL);
    assert_eq!(o.api_key, "k");
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
    Organizer::OpenAi(
        OpenAiOrganizer::new(&OpenAiConfig {
            base_url,
            api_key: "sk-test".into(),
            model: "m".into(),
            prompt: None,
        })
        .unwrap(),
    )
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
