//! What a connection needs, fetched at run time and kept in memory only.

use std::fmt;
use std::time::{Duration, Instant};

use base64::Engine as _;

/// Credentials are renewed this long before their token expires (it lasts
/// 24 h), or this long after fetching if the token's expiry is unreadable.
const RENEW_EARLY: Duration = Duration::from_secs(10 * 60);
const RENEW_BLIND: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub(super) struct Credentials {
    pub device_id: String,
    /// From the remote settings; names the IME to the service.
    pub app_key: String,
    /// A JWT, traded for the app key. Secret.
    pub sami_token: String,
    /// When to fetch new ones.
    pub renew_at: Instant,
}

impl Credentials {
    pub fn new(device_id: String, app_key: String, sami_token: String) -> Self {
        let renew_at = Instant::now() + renew_in(&sami_token);
        Self {
            device_id,
            app_key,
            sami_token,
            renew_at,
        }
    }

    pub fn is_fresh(&self) -> bool {
        Instant::now() < self.renew_at
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("app_key", &self.app_key)
            .field("sami_token", &"<redacted>")
            .field("renew_at", &self.renew_at)
            .finish()
    }
}

/// How long credentials with this token are good for: until shortly before
/// the expiry written in it, measured from now so the Mac's clock does not
/// matter.
fn renew_in(token: &str) -> Duration {
    let lifetime = token
        .split('.')
        .nth(1)
        .and_then(|claims| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims.trim_end_matches('='))
                .ok()
        })
        .and_then(|json| serde_json::from_slice::<serde_json::Value>(&json).ok())
        .and_then(|claims| Some(claims.get("exp")?.as_i64()? - claims.get("iat")?.as_i64()?))
        .and_then(|secs| u64::try_from(secs).ok())
        .map(Duration::from_secs);
    match lifetime {
        Some(l) if l > RENEW_EARLY * 2 => l - RENEW_EARLY,
        Some(l) => l / 2,
        None => RENEW_BLIND,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: serde_json::Value) -> String {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("h.{}.s", b64.encode(claims.to_string()))
    }

    #[test]
    fn renewal_follows_the_token_lifetime() {
        let day = jwt(serde_json::json!({"iat": 1000, "exp": 1000 + 86_400}));
        assert_eq!(renew_in(&day), Duration::from_secs(86_400) - RENEW_EARLY);
        let short = jwt(serde_json::json!({"iat": 0, "exp": 600}));
        assert_eq!(renew_in(&short), Duration::from_secs(300));
        assert_eq!(renew_in("not a jwt"), RENEW_BLIND);
    }
}
