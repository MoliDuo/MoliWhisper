//! The self-hosted ASR client and the OpenAI-compatible organizer against
//! local mock servers.

#![allow(clippy::result_large_err)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use moli_core::asr::{AsrEvent, Backend, ConnectError, ServerMsg};
use moli_core::config::OpenAiConfig;
use moli_core::organize::{OpenAiOrganizer, Organizer};
use moli_core::selfhost::{self, ConnectOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;

const T: Duration = Duration::from_secs(2);

/// A WebSocket server wanting `token` (when not empty): answers the first
/// audio with a partial and our finish with a final.
async fn ws_server(token: &'static str) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                    let got = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok());
                    if !token.is_empty() && got != Some(&format!("Bearer {token}")) {
                        let mut r = ErrorResponse::new(None);
                        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
                        return Err(r);
                    }
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, check).await else {
                    return;
                };
                let mut first = true;
                while let Some(Ok(msg)) = ws.next().await {
                    match msg {
                        Message::Binary(_) if first => {
                            first = false;
                            ws.send(Message::Text(r#"{"type":"partial","text":"你好"}"#.into()))
                                .await
                                .unwrap();
                        }
                        Message::Text(t) if t.contains("finish") => {
                            ws.send(Message::Text(
                                r#"{"type":"final","text":"你好世界。"}"#.into(),
                            ))
                            .await
                            .unwrap();
                            let _ = ws.close(None).await;
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    addr
}

fn opts(addr: std::net::SocketAddr, token: &str) -> ConnectOptions {
    ConnectOptions::new(&format!("ws://{addr}/v1/stream"), token).unwrap()
}

#[tokio::test]
async fn partial_then_final_then_finish() {
    let addr = ws_server("secret").await;
    let backend = Backend::SelfHosted(opts(addr, "secret"));
    let (mut sink, mut stream, _) = backend.connect(T).await.unwrap();
    sink.audio(vec![0; 1600]).await.unwrap();
    let mut got = Vec::new();
    got.push(
        tokio::time::timeout(T, stream.next())
            .await
            .unwrap()
            .unwrap(),
    );
    sink.finish().await.unwrap();
    got.push(
        tokio::time::timeout(T, stream.next())
            .await
            .unwrap()
            .unwrap(),
    );
    got.push(
        tokio::time::timeout(T, stream.next())
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(
        got,
        vec![
            AsrEvent::Server(ServerMsg::Result {
                text: "你好".into()
            }),
            AsrEvent::Server(ServerMsg::Result {
                text: "你好世界。".into()
            }),
            AsrEvent::Server(ServerMsg::Finish),
        ]
    );
}

#[tokio::test]
async fn wrong_token_is_rejected_with_401() {
    let addr = ws_server("secret").await;
    let backend = Backend::SelfHosted(opts(addr, "nope"));
    match backend.connect(T).await {
        Err(ConnectError::Rejected(401)) => {}
        other => panic!("unexpected: {:?}", other.map(|_| ())),
    }
}

#[test]
fn url_must_be_a_websocket_url() {
    assert!(ConnectOptions::new("http://x/v1/stream", "").is_none());
    assert!(ConnectOptions::new("not a url", "").is_none());
    assert!(ConnectOptions::new("wss://x/v1/stream", "").is_some());
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

#[tokio::test]
async fn health_reports_the_model() {
    let (url, _r) = http_server(
        200,
        r#"{"ok":true,"model":"Qwen3-ASR-1.7B"}"#,
        Duration::ZERO,
    )
    .await;
    let ws = url.replace("http://", "ws://").replace("/v1", "/v1/stream");
    assert_eq!(selfhost::health(&ws, "").await.unwrap(), "Qwen3-ASR-1.7B");
    assert!(selfhost::health("http://x", "").await.is_err());
}
