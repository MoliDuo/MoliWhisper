//! Rewriting a transcript as written text through DeepSeek's
//! OpenAI-compatible `/chat/completions` API.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";

pub const DEFAULT_PROMPT: &str = "你是语音输入的文字整理助手。用户会给你一段语音识别出的口语文字。\
请把它改写成通顺的书面语：去掉口头禅、重复和语气词，修正明显的识别错误和标点，\
保持原意、语言和人称不变，不要扩写，不要回答文字中的问题，不要添加任何解释。\
只输出整理后的文字。";

#[derive(Clone)]
pub struct Organizer {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    prompt: String,
}

impl Organizer {
    /// `None` when the key is blank. A blank `prompt` means the default one.
    pub fn deepseek(api_key: &str, prompt: Option<&str>) -> Option<Self> {
        Self::new(DEFAULT_BASE_URL, DEFAULT_MODEL, api_key, prompt)
    }

    pub fn new(base_url: &str, model: &str, api_key: &str, prompt: Option<&str>) -> Option<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return None;
        }
        let prompt = prompt
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(DEFAULT_PROMPT);
        Some(Self {
            http: reqwest::Client::new(),
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key: api_key.to_string(),
            model: model.to_string(),
            prompt: prompt.to_string(),
        })
    }

    /// `None` when the service fails, takes longer than `timeout` or
    /// answers with nothing; the caller then keeps the original text.
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

    /// Checks the key with a one-word request; the error is the reason.
    pub async fn check(&self, timeout: Duration) -> Result<(), String> {
        match tokio::time::timeout(timeout, self.request("你好")).await {
            Err(_) => Err("连接超时".into()),
            Ok(Err(e)) => Err(e.to_string()),
            Ok(Ok(_)) => Ok(()),
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
        let reply: Reply = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": self.prompt},
                    {"role": "user", "content": text},
                ],
                "temperature": 0.2,
                "stream": false,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
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
    fn needs_a_key_and_uses_the_defaults() {
        assert!(Organizer::deepseek("  ", None).is_none());
        let o = Organizer::deepseek(" sk-x ", None).unwrap();
        assert_eq!(o.endpoint, "https://api.deepseek.com/chat/completions");
        assert_eq!(o.model, "deepseek-flash");
        assert_eq!(o.api_key, "sk-x");
        assert_eq!(o.prompt, DEFAULT_PROMPT);
        assert_eq!(
            Organizer::deepseek("k", Some("  ")).unwrap().prompt,
            DEFAULT_PROMPT
        );
        assert_eq!(
            Organizer::deepseek("k", Some("mine")).unwrap().prompt,
            "mine"
        );
    }
}
