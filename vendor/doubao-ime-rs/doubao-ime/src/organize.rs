//! 文字整理：口语文本 → 书面文本。
//!
//! `POST /api/v2/ai/text_organization`，无需鉴权。请求体 `{"scene":6,"query":"..."}`，
//! 可选 `"stream":true` 走 SSE。

use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::ORGANIZE_SCENE;
use crate::error::{Error, Result};
use crate::Client;

/// 同步整理结果。
#[derive(Debug, Clone, Deserialize)]
pub struct Organized {
    /// 整理后的文本。
    pub content: String,
    /// 服务端判定无需改写时为 `true`。
    #[serde(default)]
    pub no_rewrite: bool,
}

/// SSE 流事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrganizeEvent {
    /// 增量片段。
    Delta(String),
    /// 最终全文。
    Completed(String),
}

impl Client {
    /// 整理文本，返回整理后的字符串。
    pub async fn organize(&self, text: &str) -> Result<Organized> {
        self.organize_scene(text, ORGANIZE_SCENE).await
    }

    /// 指定场景枚举值整理。
    pub async fn organize_scene(&self, text: &str, scene: i32) -> Result<Organized> {
        let v: Value = self
            .http
            .post(&self.endpoints().organize)
            .json(&json!({"scene": scene, "query": text}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => Ok(serde_json::from_value(v.get("data").cloned().ok_or_else(
                || Error::Api {
                    code: 0,
                    message: "缺少 data".into(),
                },
            )?)?),
            other => Err(Error::Api {
                code: other.unwrap_or(-1),
                message: v
                    .get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
            }),
        }
    }

    /// SSE 流式整理，逐段产出 [`OrganizeEvent`]。
    pub async fn organize_stream(
        &self,
        text: &str,
    ) -> Result<impl Stream<Item = Result<OrganizeEvent>>> {
        self.organize_stream_scene(text, ORGANIZE_SCENE).await
    }

    /// 指定场景的 SSE 流式整理。
    pub async fn organize_stream_scene(
        &self,
        text: &str,
        scene: i32,
    ) -> Result<impl Stream<Item = Result<OrganizeEvent>>> {
        let resp = self
            .http
            .post(&self.endpoints().organize)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&json!({"scene": scene, "query": text, "stream": true}))
            .send()
            .await?
            .error_for_status()?;
        Ok(sse_stream(resp.bytes_stream()))
    }
}

fn sse_stream(
    mut bytes: impl Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
) -> impl Stream<Item = Result<OrganizeEvent>> {
    use futures_util::StreamExt;
    async_stream::stream! {
        let mut buf = String::new();
        let mut event: Option<String> = None;
        while let Some(chunk) = bytes.next().await {
            let chunk = match chunk { Ok(c) => c, Err(e) => { yield Err(e.into()); return; } };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end_matches('\r').to_owned();
                buf.drain(..=nl);
                if let Some(rest) = line.strip_prefix("event:") {
                    event = Some(rest.trim().to_owned());
                } else if let Some(rest) = line.strip_prefix("data:") {
                    let payload = rest.trim();
                    if payload == "[DONE]" { return; }
                    match parse_sse_data(event.as_deref(), payload) {
                        Ok(Some(ev)) => yield Ok(ev),
                        Ok(None) => {}
                        Err(e) => { yield Err(e); return; }
                    }
                } else if line.is_empty() {
                    event = None;
                }
            }
        }
    }
}

fn parse_sse_data(event: Option<&str>, payload: &str) -> Result<Option<OrganizeEvent>> {
    let v: Value = serde_json::from_str(payload)?;
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(match event {
        Some("scene.delta") => Some(OrganizeEvent::Delta(content)),
        Some("scene.completed") => Some(OrganizeEvent::Completed(content)),
        _ => None,
    })
}
