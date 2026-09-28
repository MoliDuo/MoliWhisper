//! keyhub 握手：ECDSA 签名 + ECDHE 密钥交换，获取信封加密票据（x-tt-e-k）。
//!
//! ```text
//! POST /handshake
//! Header: x-tt-s-sign: base64(DER(ECDSA-P256-SHA256(ClientHello_JSON)))
//! Body  : ClientHello JSON（紧凑序列化，字段顺序固定）
//! ```
//! 会话密钥 = ECDH(客户端私钥, 服务端 key_share.pubkey)。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::{ecdh::diffie_hellman, PublicKey};
use rand::rngs::OsRng;
use serde_json::{json, Value};

use crate::config::AID;
use crate::error::{Error, Result};
use crate::Client;

/// 握手结果。
#[derive(Debug, Clone)]
pub struct Handshake {
    /// 短期票据（约 3 小时）。
    pub ticket: String,
    /// 短期票据有效期（秒）。
    pub ticket_exp: Option<i64>,
    /// 长期票据（约 3 天）。
    pub ticket_long: Option<String>,
    /// 长期票据有效期（秒）。
    pub ticket_long_exp: Option<i64>,
    /// ECDH 会话密钥（派生成功时）。
    pub session_key: Option<Vec<u8>>,
    /// 服务端证书 subject。
    pub cert_subject: Option<String>,
    /// 服务端证书 issuer。
    pub cert_issuer: Option<String>,
}

impl Client {
    /// 执行一次 keyhub 握手。
    pub async fn keyhub_handshake(&self, device_id: &str) -> Result<Handshake> {
        let signing = SigningKey::random(&mut OsRng);
        let pub_uncompressed = signing.verifying_key().to_encoded_point(false);
        let pub_b64 = B64.encode(pub_uncompressed.as_bytes());
        let mut random32 = [0u8; 32];
        rand::Rng::fill(&mut OsRng, &mut random32[..]);

        // 字段顺序固定，且服务端对整段字节验签，故手工拼紧凑 JSON。
        let hello = json!({
            "version": 2,
            "random": B64.encode(random32),
            "app_id": AID.to_string(),
            "did": device_id,
            "curve": "secp256r1",
            "pubkey": pub_b64,
            "key_shares": [{"curve": "secp256r1", "pubkey": pub_b64}],
            "cipher_suites": [4097],
        });
        let body = serde_json::to_vec(&hello)?;
        let sig: Signature = signing.sign(&body);
        let sign_b64 = B64.encode(sig.to_der().as_bytes());

        let sh: Value = self
            .http
            .post(&self.endpoints().keyhub)
            .header("x-tt-s-sign", sign_b64)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let ticket = sh
            .get("ticket")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Auth("keyhub 握手未返回 ticket".into()))?
            .to_owned();

        let session_key = sh
            .pointer("/key_share/pubkey")
            .and_then(Value::as_str)
            .and_then(|s| B64.decode(s).ok())
            .and_then(|bytes| PublicKey::from_sec1_bytes(&bytes).ok())
            .map(|server_pub| diffie_hellman(signing.as_nonzero_scalar(), server_pub.as_affine()))
            .map(|secret| secret.raw_secret_bytes().to_vec());

        let (cert_subject, cert_issuer) = parse_cert(sh.get("cert").and_then(Value::as_str));

        Ok(Handshake {
            ticket,
            ticket_exp: sh.get("ticket_exp").and_then(Value::as_i64),
            ticket_long: sh
                .get("ticket_long")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ticket_long_exp: sh.get("ticket_long_exp").and_then(Value::as_i64),
            session_key,
            cert_subject,
            cert_issuer,
        })
    }
}

fn parse_cert(pem: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(pem) = pem else { return (None, None) };
    let Ok((_, der)) = x509_parser::pem::parse_x509_pem(pem.as_bytes()) else {
        return (None, None);
    };
    match der.parse_x509() {
        Ok(cert) => (
            Some(cert.subject().to_string()),
            Some(cert.issuer().to_string()),
        ),
        Err(_) => (None, None),
    }
}
