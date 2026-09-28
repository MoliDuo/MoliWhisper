//! 运行时凭据：app_key（服务端下发）→ sami_token（匿名换发）→ 可选 keyhub 票据。
//!
//! ```text
//! ① GET  TTKitchen settings/v3   -> data.settings.asr_config.app_key
//! ② POST get_config              -> Data.sami_token (JWT)
//! ③（可选）keyhub 握手            -> ticket（WS 查询参数 x-tt-e-k）
//! ```

use std::fmt;

use serde_json::{json, Value};

use crate::config::{AID, DEVICE_PLATFORM, UA, UA_MAC};
use crate::error::{Error, Result};
use crate::Client;

/// 一次 ASR 会话所需的全部要素。
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// 匿名设备 ID。
    pub device_id: String,
    /// 服务端下发的 app_key。
    pub app_key: String,
    /// SAMI JWT。
    pub sami_token: String,
    /// keyhub 票据，仅兼容模式需要。
    pub ticket: Option<String>,
    /// 票据有效期（秒）。
    pub ticket_exp: Option<i64>,
}

impl fmt::Debug for Credentials {
    // 不在日志里输出完整 token / 票据
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("app_key", &self.app_key)
            .field(
                "sami_token",
                &format_args!("<{} chars>", self.sami_token.len()),
            )
            .field(
                "ticket",
                &self.ticket.as_ref().map(|t| format!("<{} chars>", t.len())),
            )
            .field("ticket_exp", &self.ticket_exp)
            .finish()
    }
}

/// 获取凭据的选项。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialOptions {
    /// 同时执行 keyhub 握手获取票据（兼容模式需要）。
    pub with_ticket: bool,
}

impl Client {
    fn ua_for(&self, compat: bool) -> &str {
        match &self.config.user_agent {
            Some(ua) => ua,
            None if compat => UA_MAC,
            None => UA,
        }
    }

    /// ① 从 TTKitchen 获取 app_key。
    pub async fn fetch_app_key(&self, device_id: &str) -> Result<String> {
        self.fetch_app_key_ua(device_id, self.ua_for(false)).await
    }

    async fn fetch_app_key_ua(&self, device_id: &str, ua: &str) -> Result<String> {
        let aid = AID.to_string();
        let v: Value = self
            .http
            .get(&self.endpoints().ttkitchen)
            .query(&[
                ("aid", aid.as_str()),
                ("device_id", device_id),
                ("os", DEVICE_PLATFORM),
                ("channel", "release"),
            ])
            .header(reqwest::header::USER_AGENT, ua)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        v.pointer("/data/settings/asr_config/app_key")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Auth("服务端配置中缺少 asr_config.app_key".into()))
    }

    /// ② 用 app_key 换取 SAMI token。
    pub async fn fetch_sami_token(&self, app_key: &str, device_id: &str) -> Result<String> {
        self.fetch_sami_token_ua(app_key, device_id, self.ua_for(false))
            .await
    }

    async fn fetch_sami_token_ua(
        &self,
        app_key: &str,
        device_id: &str,
        ua: &str,
    ) -> Result<String> {
        let v: Value = self
            .http
            .post(&self.endpoints().sami_config)
            .header(reqwest::header::USER_AGENT, ua)
            .json(&json!({"sami_app_key": app_key, "device_id": device_id, "aid": AID}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let data = v
            .get("Data")
            .filter(|d| !d.is_null())
            .or_else(|| v.get("data"));
        data.and_then(|d| d.get("sami_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Auth("get_config 响应中缺少 sami_token".into()))
    }

    /// 在线获取全部凭据要素；任一环节失败即返回错误。
    pub async fn credentials(&self, opts: CredentialOptions) -> Result<Credentials> {
        let ua = self.ua_for(opts.with_ticket).to_owned();
        let device_id = self.device_id();
        let app_key = self.fetch_app_key_ua(&device_id, &ua).await?;
        tracing::debug!(%app_key, "获取 app_key");
        let sami_token = self.fetch_sami_token_ua(&app_key, &device_id, &ua).await?;
        tracing::debug!(len = sami_token.len(), "获取 sami_token");
        let mut creds = Credentials {
            device_id,
            app_key,
            sami_token,
            ticket: None,
            ticket_exp: None,
        };
        if opts.with_ticket {
            let hs = self.keyhub_handshake(&creds.device_id).await?;
            tracing::debug!(exp = ?hs.ticket_exp, "获取 keyhub 票据");
            creds.ticket_exp = hs.ticket_exp;
            creds.ticket = Some(hs.ticket);
        }
        Ok(creds)
    }
}
