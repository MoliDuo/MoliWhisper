//! The service's HTTP APIs: credentials, text cleanup, releases.
//!
//! ```text
//! GET  settings     ?aid&device_id&os&channel -> data.settings.asr_config.app_key
//! POST sami_config  {sami_app_key, device_id, aid} -> Data.sami_token
//! POST organize     {scene, query} -> {code, msg, data: {content, no_rewrite}}
//! GET  version      ?aid&platform&channel&version_code -> {code, data: {list}}
//! ```

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use super::client::ImeClient;
use super::error::ApiError;
use super::{AID, DEVICE_PLATFORM};

/// The IME release this client presents itself as.
pub const IME_VERSION_CODE: u64 = 1_000_103;
/// What the IME's voice input asks the rewrite for.
const ORGANIZE_SCENE: i32 = 6;

/// An IME release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub name: String,
    pub code: u64,
}

impl ImeClient {
    /// The app key the remote settings hand this device.
    pub(super) async fn fetch_app_key(&self, device_id: &str) -> Result<String, ApiError> {
        let aid = AID.to_string();
        let reply: Value = self
            .inner
            .http
            .get(&self.inner.endpoints.settings)
            .query(&[
                ("aid", aid.as_str()),
                ("device_id", device_id),
                ("os", DEVICE_PLATFORM),
                ("channel", "release"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        reply
            .pointer("/data/settings/asr_config/app_key")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or(ApiError::Malformed("no asr_config.app_key in the settings"))
    }

    /// Trades an app key for a token.
    pub(super) async fn fetch_sami_token(
        &self,
        app_key: &str,
        device_id: &str,
    ) -> Result<String, ApiError> {
        let reply: Value = self
            .inner
            .http
            .post(&self.inner.endpoints.sami_config)
            .json(&json!({"sami_app_key": app_key, "device_id": device_id, "aid": AID}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // Spelled either way.
        let data = reply
            .get("Data")
            .filter(|d| !d.is_null())
            .or_else(|| reply.get("data"));
        data.and_then(|d| d.get("sami_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or(ApiError::Malformed("no sami_token in the config"))
    }

    /// Rewrites spoken text as written text. `None` when the service fails,
    /// takes longer than `timeout`, or answers with nothing.
    pub async fn organize(&self, text: &str, timeout: Duration) -> Option<String> {
        let t = Instant::now();
        match tokio::time::timeout(timeout, self.request_organize(text)).await {
            Err(_) => {
                tracing::warn!("organizing timed out");
                None
            }
            Ok(Err(e)) => {
                tracing::warn!("organizing failed: {e}");
                None
            }
            Ok(Ok(out)) => {
                let content = out.content.trim();
                tracing::info!(
                    ms = t.elapsed().as_millis(),
                    no_rewrite = out.no_rewrite,
                    "organized"
                );
                (!content.is_empty()).then(|| content.to_string())
            }
        }
    }

    async fn request_organize(&self, text: &str) -> Result<Organized, ApiError> {
        #[derive(Deserialize)]
        struct Reply {
            code: Option<i64>,
            msg: Option<String>,
            data: Option<Organized>,
        }
        let reply: Reply = self
            .inner
            .http
            .post(&self.inner.endpoints.organize)
            .json(&json!({"scene": ORGANIZE_SCENE, "query": text}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match reply.code {
            Some(0) => reply
                .data
                .ok_or(ApiError::Malformed("no data in the rewrite")),
            code => Err(ApiError::Service {
                code: code.unwrap_or(-1),
                message: reply.msg.unwrap_or_else(|| "unknown".into()),
            }),
        }
    }

    /// The latest IME release if it is newer than the one this client
    /// presents itself as: the service may one day turn old ones away.
    /// `None` when it is not newer or cannot be told.
    pub async fn newer_version(&self, timeout: Duration) -> Option<Release> {
        #[derive(Deserialize)]
        struct Reply {
            code: i64,
            data: Option<Data>,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(default)]
            list: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            version_name: Option<String>,
            version_code: Option<u64>,
        }
        let aid = AID.to_string();
        let code = IME_VERSION_CODE.to_string();
        let query = [
            ("aid", aid.as_str()),
            ("platform", "macos"),
            ("channel", "release"),
            ("version_code", code.as_str()),
        ];
        let reply: Reply = async {
            self.inner
                .http
                .get(&self.inner.endpoints.version)
                .query(&query)
                .timeout(timeout)
                .send()
                .await?
                .json()
                .await
        }
        .await
        .inspect_err(|e| tracing::debug!("IME version check failed: {e}"))
        .ok()?;
        if reply.code != 0 {
            tracing::debug!(code = reply.code, "IME version check refused");
            return None;
        }
        let latest = reply
            .data?
            .list
            .into_iter()
            .filter_map(|r| {
                Some(Release {
                    name: r.version_name.unwrap_or_default(),
                    code: r.version_code?,
                })
            })
            .max_by_key(|r| r.code)?;
        (latest.code > IME_VERSION_CODE).then_some(latest)
    }
}

/// The rewrite.
#[derive(Deserialize)]
struct Organized {
    content: String,
    /// The service found nothing to change.
    #[serde(default)]
    no_rewrite: bool,
}
