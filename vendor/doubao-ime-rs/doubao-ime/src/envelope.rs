//! TTNet（TNC）配置拉取与响应信封加密。
//!
//! 信封加密：请求头 `x-tt-e-t: <bk_t>` 启用；响应头 `X-Tt-E-P` = base64(12B nonce)，
//! 响应体为密文；解密 = ChaCha20(key=bk_k, nonce = 00000000 || X-Tt-E-P)。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
use regex::bytes::Regex;
use serde_json::{json, Value};

use crate::config::*;
use crate::error::{Error, Result};
use crate::Client;

/// TNC 配置中标志「完整配置」的键。
pub const REQUIRED_KEY: &str = "encrypt_config_v1";

/// TNC 全量配置拉取结果。
#[derive(Debug, Clone)]
pub struct TncConfig {
    /// 完整响应 JSON。
    pub raw: Value,
    /// 命中的节点主机。
    pub host: String,
}

impl TncConfig {
    /// `data` 对象。
    pub fn data(&self) -> Option<&Value> {
        self.raw.get("data")
    }
    /// 解析出信封配置。
    pub fn envelope(&self) -> Result<EnvelopeConfig> {
        EnvelopeConfig::from_tnc(&self.raw)
    }
}

/// 信封加密配置。
#[derive(Debug, Clone)]
pub struct EnvelopeConfig {
    /// ChaCha20 密钥（32B）。
    pub bk_k: Option<[u8; 32]>,
    /// 备用票据（DER，base64）。
    pub bk_t: Option<String>,
    /// keyhub URL。
    pub url: Option<String>,
}

impl EnvelopeConfig {
    /// 从 TNC 完整响应构造。
    pub fn from_tnc(config: &Value) -> Result<Self> {
        let ec = config
            .pointer(&format!("/data/{REQUIRED_KEY}"))
            .ok_or_else(|| Error::Config(format!("配置中缺少 {REQUIRED_KEY}")))?;
        let bk_k = ec
            .get("bk_k")
            .and_then(Value::as_str)
            .map(|s| B64.decode(s))
            .transpose()
            .map_err(|e| Error::Crypto(format!("bk_k base64 解码失败: {e}")))?
            .map(|v| {
                <[u8; 32]>::try_from(v.as_slice())
                    .map_err(|_| Error::Crypto("bk_k 长度不是 32 字节".into()))
            })
            .transpose()?;
        Ok(Self {
            bk_k,
            bk_t: ec.get("bk_t").and_then(Value::as_str).map(str::to_owned),
            url: ec.get("url").and_then(Value::as_str).map(str::to_owned),
        })
    }
}

/// 信封响应。
#[derive(Debug, Clone)]
pub struct EnvelopeResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 已解密的明文。
    pub body: Vec<u8>,
    /// 响应是否经过信封加密。
    pub encrypted: bool,
}

impl EnvelopeResponse {
    /// 明文按 UTF-8 解读。
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

/// 解密信封响应体。
pub fn decrypt_envelope(bk_k: &[u8; 32], ep_b64: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
    let ep = B64
        .decode(ep_b64)
        .map_err(|e| Error::Crypto(format!("X-Tt-E-P base64: {e}")))?;
    let nonce: [u8; 12] = ep
        .as_slice()
        .try_into()
        .map_err(|_| Error::Crypto(format!("X-Tt-E-P 长度异常: {}", ep.len())))?;
    // 原实现的 16 字节 nonce = 00000000 || 12B，正是 IETF ChaCha20 的
    // (初始计数器 0) || (12B nonce)，故这里直接用 12B nonce、计数器从 0 开始。
    let mut cipher = ChaCha20::new(bk_k.into(), (&nonce).into());
    let mut out = ciphertext.to_vec();
    cipher.apply_keystream(&mut out);
    Ok(out)
}

impl Client {
    /// 拉取 TTNet 全量配置（依次尝试各节点，返回首个含 `encrypt_config_v1` 的响应）。
    pub async fn fetch_tnc_config(
        &self,
        cronet_version: &str,
        ttnet_version: &str,
    ) -> Result<TncConfig> {
        let device_id = self.device_id();
        let aid = AID.to_string();
        let query: Vec<(&str, &str)> = vec![
            ("aid", &aid),
            ("device_id", &device_id),
            ("device_platform", DEVICE_PLATFORM),
            ("version_code", "1"),
            ("channel", "release"),
            ("update_version_code", "1000103"),
            ("device_type", "Mac"),
            ("app_name", "DoubaoIme"),
            ("cronet_version", cronet_version),
            ("ttnet_version", ttnet_version),
        ];
        let mut errors = Vec::new();
        for base in &self.endpoints().tnc {
            let host = url::Url::parse(base)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            match self
                .http
                .get(base)
                .query(&query)
                .header(reqwest::header::USER_AGENT, UA)
                .send()
                .await
            {
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => {
                        let v: Value = resp.json().await?;
                        if v.pointer(&format!("/data/{REQUIRED_KEY}")).is_some() {
                            return Ok(TncConfig { raw: v, host });
                        }
                        let n = v
                            .get("data")
                            .and_then(Value::as_object)
                            .map(|o| o.len())
                            .unwrap_or(0);
                        errors.push(format!("{host}: 缺少 {REQUIRED_KEY}（{n} 键）"));
                    }
                    Err(e) => errors.push(format!("{host}: {e}")),
                },
                Err(e) => errors.push(format!("{host}: {e}")),
            }
        }
        Err(Error::Config(format!(
            "所有 TNC 节点均未返回完整配置:\n  {}",
            errors.join("\n  ")
        )))
    }

    /// 以信封加密方式发送 POST，并自动解密响应。
    pub async fn envelope_post(
        &self,
        url: &str,
        body: Vec<u8>,
        cfg: &EnvelopeConfig,
    ) -> Result<EnvelopeResponse> {
        let bk_t = cfg
            .bk_t
            .as_deref()
            .ok_or_else(|| Error::Config("信封配置缺少 bk_t".into()))?;
        let resp = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("x-tt-e-t", bk_t)
            .body(body)
            .send()
            .await?
            .error_for_status()?;
        let status = resp.status().as_u16();
        let ep = resp
            .headers()
            .get("X-Tt-E-P")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let raw = resp.bytes().await?.to_vec();
        match (ep, cfg.bk_k) {
            (Some(ep), Some(key)) => Ok(EnvelopeResponse {
                status,
                body: decrypt_envelope(&key, &ep, &raw)?,
                encrypted: true,
            }),
            _ => Ok(EnvelopeResponse {
                status,
                body: raw,
                encrypted: false,
            }),
        }
    }

    /// 以信封加密方式调用文字整理。
    pub async fn envelope_organize(
        &self,
        text: &str,
        cfg: &EnvelopeConfig,
    ) -> Result<EnvelopeResponse> {
        let body = serde_json::to_vec(&json!({"scene": ORGANIZE_SCENE, "query": text}))?;
        self.envelope_post(&self.endpoints().organize.clone(), body, cfg)
            .await
    }
}

/// 从 libsscronet.dylib 提取 (cronet_version, ttnet_version)。
///
/// 库中三个常量字符串相邻：`<8位hex>\0<YYYY-MM-DD>\0...<x.y.z.w-xxx>\0`。
pub fn extract_versions_from_lib(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let ttnet_re = Regex::new(r"[0-9]+(?:\.[0-9]+){2,4}-[a-z0-9]+\x00").unwrap();
    let Some(m) = ttnet_re.find(bytes) else {
        return (None, None);
    };
    let ttnet = String::from_utf8_lossy(&bytes[m.start()..m.end() - 1]).into_owned();
    let back = &bytes[m.start().saturating_sub(128)..m.start()];
    let hash = Regex::new(r"\b[0-9a-f]{8}\x00")
        .unwrap()
        .find_iter(back)
        .last();
    let date = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}\x00")
        .unwrap()
        .find_iter(back)
        .last();
    let cronet = match (hash, date) {
        (Some(h), Some(d)) => Some(format!(
            "{}_{}",
            String::from_utf8_lossy(&back[h.start()..h.end() - 1]),
            String::from_utf8_lossy(&back[d.start()..d.end() - 1]),
        )),
        _ => None,
    };
    (cronet, Some(ttnet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypt_roundtrip() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let plain = b"{\"content\":\"hi\"}".to_vec();
        let mut ct = plain.clone();
        ChaCha20::new(&key.into(), (&nonce).into()).apply_keystream(&mut ct);
        let out = decrypt_envelope(&key, &B64.encode(nonce), &ct).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn envelope_config_parse() {
        let key = B64.encode([9u8; 32]);
        let cfg = EnvelopeConfig::from_tnc(&json!({
            "data": {REQUIRED_KEY: {"bk_k": key, "bk_t": "VA==", "url": "u"}}
        }))
        .unwrap();
        assert_eq!(cfg.bk_k, Some([9u8; 32]));
        assert_eq!(cfg.bk_t.as_deref(), Some("VA=="));
    }

    #[test]
    fn version_extraction() {
        let lib = b"junk\x00e8646bd5\x002026-09-18\x00x\x004.2.243.39-doubao\x00tail";
        let (c, t) = extract_versions_from_lib(lib);
        assert_eq!(c.as_deref(), Some("e8646bd5_2026-09-18"));
        assert_eq!(t.as_deref(), Some("4.2.243.39-doubao"));
    }
}
