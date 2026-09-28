//! 用本地假 WebSocket 服务器端到端测试 ASR 会话（不访问真实网络）。

use std::net::SocketAddr;

use doubao_ime::auth::Credentials;
use doubao_ime::proto::{event, WebSocketRequest, WebSocketResponse};
use doubao_ime::{AsrOptions, Client, Endpoints, Error};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    Ok,
    Fail,
    Silent, // 收到 FinishSession 不回复，测试超时路径
}

struct Seen {
    events: Vec<String>,
    path: String,
    audio_len: usize,
    resource_id: Option<String>,
}

async fn spawn_server(behavior: Behavior) -> (SocketAddr, oneshot::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut path = String::new();
        let mut resource_id = None;
        let ws = tokio_tungstenite::accept_hdr_async(
            stream,
            |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                path = req.uri().to_string();
                resource_id = req
                    .headers()
                    .get("X-Api-Resource-Id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                Ok(resp)
            },
        )
        .await
        .unwrap();
        let (mut w, mut r) = ws.split();

        let mut events = Vec::new();
        let mut audio_len = 0usize;
        let mut n = 0;

        let result = |text: &str| {
            Message::Binary(
                WebSocketResponse {
                    payload: json!({"results":[{"text":text}]}).to_string(),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
        };

        while let Some(Ok(msg)) = r.next().await {
            let Message::Binary(data) = msg else { continue };
            let req = WebSocketRequest::decode(data.as_slice()).unwrap();
            events.push(req.event.clone());
            match req.event.as_str() {
                event::START_TASK => {
                    w.send(Message::Binary(
                        WebSocketResponse {
                            event: event::TASK_STARTED.into(),
                            task_id: "task-1".into(),
                            ..Default::default()
                        }
                        .encode_to_vec(),
                    ))
                    .await
                    .unwrap();
                }
                event::START_SESSION => {
                    assert_eq!(req.token, "TOK");
                    assert_eq!(req.namespace, "ASR");
                    w.send(Message::Binary(
                        WebSocketResponse {
                            event: event::SESSION_STARTED.into(),
                            ..Default::default()
                        }
                        .encode_to_vec(),
                    ))
                    .await
                    .unwrap();
                }
                event::TASK_REQUEST => {
                    audio_len += req.audio_data.len();
                    n += 1;
                    if behavior == Behavior::Fail {
                        w.send(Message::Binary(
                            WebSocketResponse {
                                event: event::SESSION_FAILED.into(),
                                status_code: 40_000_012,
                                status_text: "bad data".into(),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        ))
                        .await
                        .unwrap();
                    } else {
                        w.send(result(if n == 1 { "你好" } else { "你好世界" }))
                            .await
                            .unwrap();
                    }
                }
                event::FINISH_SESSION => {
                    if behavior == Behavior::Ok {
                        w.send(result("你好世界。")).await.unwrap();
                        w.send(Message::Binary(
                            WebSocketResponse {
                                event: event::SESSION_FINISHED.into(),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        ))
                        .await
                        .unwrap();
                    }
                }
                event::FINISH_TASK => {
                    w.send(Message::Binary(
                        WebSocketResponse {
                            event: event::TASK_FINISHED.into(),
                            ..Default::default()
                        }
                        .encode_to_vec(),
                    ))
                    .await
                    .ok();
                    break;
                }
                _ => {}
            }
        }
        let _ = tx.send(Seen {
            events,
            path,
            audio_len,
            resource_id,
        });
    });

    (addr, rx)
}

use prost::Message as _;

fn client_for(addr: SocketAddr) -> Client {
    let mut ep = Endpoints::default();
    ep.asr_ws = format!("ws://{addr}/ocean/api/v1/ws");
    Client::builder().endpoints(ep).build().unwrap()
}

fn creds() -> Credentials {
    Credentials {
        device_id: "1234567890123456".into(),
        app_key: "AK".into(),
        sami_token: "TOK".into(),
        ticket: Some("TK/+=".into()),
        ticket_exp: Some(10800),
    }
}

fn fast_opts() -> AsrOptions {
    AsrOptions {
        send_interval: std::time::Duration::ZERO,
        finish_timeout: std::time::Duration::from_secs(2),
        ..Default::default()
    }
}

#[tokio::test]
async fn session_ok_streams_partials_and_final() {
    let (addr, seen_rx) = spawn_server(Behavior::Ok).await;
    let client = client_for(addr);
    let mut session = client.asr_session(creds(), fast_opts()).await.unwrap();
    let mut partials = session.partials();
    let collector = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(t) = partials.recv().await {
            v.push(t);
        }
        v
    });
    session.send_audio(&vec![0u8; 12_800]).await.unwrap();
    let text = session.finish().await.unwrap();
    assert_eq!(text, "你好世界。");

    let partials = collector.await.unwrap();
    assert_eq!(partials, vec!["你好", "你好世界", "你好世界。"]);

    let seen = seen_rx.await.unwrap();
    assert_eq!(seen.audio_len, 12_800);
    assert_eq!(&seen.events[..2], &["StartTask", "StartSession"]);
    assert_eq!(
        seen.events.iter().filter(|e| *e == "TaskRequest").count(),
        2
    );
    assert!(seen.events.iter().any(|e| e == "FinishSession"));
    assert!(!seen.path.contains("x-tt-e-k"));
    assert_eq!(seen.resource_id.as_deref(), Some("original.sami.ASR"));
}

#[tokio::test]
async fn compat_mode_sets_query_and_headers() {
    let (addr, seen_rx) = spawn_server(Behavior::Ok).await;
    let client = client_for(addr);
    let opts = AsrOptions {
        compat: true,
        ..fast_opts()
    };
    let mut session = client.asr_session(creds(), opts).await.unwrap();
    session.send_audio(&vec![0u8; 100]).await.unwrap();
    session.finish().await.unwrap();
    let seen = seen_rx.await.unwrap();
    assert!(
        seen.path.contains("x-tt-e-k=TK%2F%2B%3D"),
        "path = {}",
        seen.path
    );
    assert!(seen.path.contains("sdk-version=2"));
}

#[tokio::test]
async fn finish_timeout_keeps_partial_text() {
    let (addr, seen_rx) = spawn_server(Behavior::Silent).await;
    let client = client_for(addr);
    let opts = AsrOptions {
        finish_timeout: std::time::Duration::from_millis(300),
        ..fast_opts()
    };
    let mut session = client.asr_session(creds(), opts).await.unwrap();
    session.send_audio(&vec![0u8; 100]).await.unwrap();
    let text = session.finish().await.unwrap();
    assert_eq!(text, "你好");
    drop(seen_rx);
}

#[tokio::test]
async fn server_failure_becomes_error() {
    let (addr, seen_rx) = spawn_server(Behavior::Fail).await;
    let client = client_for(addr);
    let mut session = client.asr_session(creds(), fast_opts()).await.unwrap();
    // 失败帧可能在 send_audio 的顺带接收阶段到达，也可能延后到 finish；
    // 两者都算通过，只要会话最终以 Asr 错误结束。
    let err = match session.send_audio(&vec![0u8; 100]).await {
        Err(e) => e,
        Ok(()) => session.finish().await.unwrap_err(),
    };
    assert!(matches!(err, Error::Asr(_)), "got {err:?}");
    assert!(err.to_string().contains("SessionFailed"));
    drop(seen_rx);
}

#[tokio::test]
async fn compat_without_ticket_is_rejected() {
    let client = Client::new().unwrap();
    let c = Credentials {
        ticket: None,
        ..creds()
    };
    let err = client
        .asr_session(
            c,
            AsrOptions {
                compat: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Config(_)));
}
