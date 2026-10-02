//! Organizing through an OpenAI-compatible `/chat/completions` API.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::config::OpenAiConfig;

pub const DEFAULT_PROMPT: &str = "你是语音输入的文字整理助手。用户会给你一段语音识别出的口语文字。\
请把它改写成通顺的书面语：去掉口头禅、重复和语气词，修正明显的识别错误和标点，\
保持原意、语言和人称不变，不要扩写，不要回答文字中的问题，不要添加任何解释。\
只输出整理后的文字。";

#[derive(Clone)]
pub struct OpenAiOrganizer {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    prompt: String,
}

impl OpenAiOrganizer {
    /// `None` when the base URL or the model is missing.
    pub fn new(cfg: &OpenAiConfig) -> Option<Self> {
        let base = cfg.base_url.trim().trim_end_matches('/');
        let model = cfg.model.trim();
        if base.is_empty() || model.is_empty() {
            return None;
        }
        let prompt = cfg
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(DEFAULT_PROMPT);
        Some(Self {
            http: reqwest::Client::new(),
            endpoint: format!("{base}/chat/completions"),
            api_key: cfg.api_key.trim().to_string(),
            model: model.to_string(),
            prompt: prompt.to_string(),
        })
    }

    pub async fn organize(&self, text: &str, timeout: Duration) -> Option<String> {
        let t = Instant::now();
        match tokio::time::timeout(timeout, self.request(text)).await {
            Err(_) => {
                tracing::warn!("organizing timed out");
                None
            }
            Ok(Err(e)) => {
                tracing::warn!("organizing failed: {e}");
                None
            }
            Ok(Ok(out)) => {
                tracing::info!(ms = t.elapsed().as_millis(), "organized");
                (!out.is_empty()).then_some(out)
            }
        }
    }

    async fn request(&self, text: &str) -> Result<String, reqwest::Error> {
        #[derive(Deserialize)]
        struct Reply {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: Message,
        }
        #[derive(Deserialize)]
        struct Message {
            content: Option<String>,
        }
        let mut req = self.http.post(&self.endpoint).json(&json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.prompt},
                {"role": "user", "content": text},
            ],
            "temperature": 0.2,
            "stream": false,
        }));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let reply: Reply = req.send().await?.error_for_status()?.json().await?;
        let content = reply
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        Ok(clean(&content))
    }
}

/// Trims, and drops a leading `<think>…</think>` block that reasoning models
/// put in the content.
fn clean(content: &str) -> String {
    let content = content.trim();
    let content = match content.strip_prefix("<think>") {
        Some(rest) => rest.split_once("</think>").map_or("", |(_, after)| after),
        None => content,
    };
    content.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_block() {
        assert_eq!(clean("  <think>hmm\nok</think>\n结果。 "), "结果。");
        assert_eq!(clean("结果。"), "结果。");
        assert_eq!(clean("<think>never closed"), "");
    }

    #[test]
    fn needs_base_url_and_model() {
        let mut cfg = OpenAiConfig::default();
        assert!(OpenAiOrganizer::new(&cfg).is_none());
        cfg.base_url = "https://x/v1/".into();
        assert!(OpenAiOrganizer::new(&cfg).is_none());
        cfg.model = "m".into();
        let o = OpenAiOrganizer::new(&cfg).unwrap();
        assert_eq!(o.endpoint, "https://x/v1/chat/completions");
    }
}
