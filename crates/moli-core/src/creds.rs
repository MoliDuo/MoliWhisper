//! Doubao login credentials: the `.doubao.com` cookies plus the two IDs the
//! web client keeps in localStorage.
//!
//! Cookie values are secrets. `Debug` redacts them, and nothing here logs them.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::asr::params::Identity;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// From localStorage `samantha_web_web_id`.web_id.
    #[serde(alias = "deviceId")]
    pub device_id: String,
    /// From localStorage `__tea_cache_tokens_497858`.web_id.
    #[serde(alias = "webId")]
    pub web_id: String,
    pub cookies: BTreeMap<String, String>,
}

impl Credentials {
    pub fn identity(&self) -> Identity<'_> {
        Identity {
            device_id: &self.device_id,
            web_id: &self.web_id,
            language: self.cookies.get("i18next").map(String::as_str),
            region: self.cookies.get("flow_user_country").map(String::as_str),
        }
    }

    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Whether the cookies needed to authenticate are present at all.
    pub fn has_session(&self) -> bool {
        ["sessionid", "sid_guard"]
            .iter()
            .all(|k| self.cookies.get(*k).is_some_and(|v| !v.is_empty()))
            && !self.device_id.is_empty()
            && !self.web_id.is_empty()
    }

    /// Unix time the session expires, from `sid_guard`.
    pub fn expires_at(&self) -> Option<u64> {
        parse_sid_guard(self.cookies.get("sid_guard")?)
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("web_id", &self.web_id)
            .field("cookies", &self.cookies.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// `sid_guard` looks like `{sessionid}|{issued_unix}|{ttl_secs}|{http date}`,
/// possibly percent-encoded. Returns `issued + ttl`.
pub fn parse_sid_guard(value: &str) -> Option<u64> {
    let decoded = value.replace("%7C", "|").replace("%7c", "|");
    let mut parts = decoded.split('|');
    let _sid = parts.next()?;
    let issued: u64 = parts.next()?.trim().parse().ok()?;
    let ttl: u64 = parts.next()?.trim().parse().ok()?;
    issued.checked_add(ttl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> Credentials {
        Credentials {
            device_id: "d".into(),
            web_id: "w".into(),
            cookies: BTreeMap::from([
                ("sessionid".into(), "SECRET1".into()),
                (
                    "sid_guard".into(),
                    "abc%7C1759000000%7C2592000%7CSat,+25-Oct-2025".into(),
                ),
                ("i18next".into(), "en".into()),
            ]),
        }
    }

    #[test]
    fn sid_guard_expiry() {
        assert_eq!(parse_sid_guard("abc|100|50|whatever"), Some(150));
        assert_eq!(creds().expires_at(), Some(1_759_000_000 + 2_592_000));
        assert_eq!(parse_sid_guard("abc"), None);
        assert_eq!(parse_sid_guard("abc|x|1"), None);
    }

    #[test]
    fn debug_redacts_cookie_values() {
        let s = format!("{:?}", creds());
        assert!(s.contains("sessionid"));
        assert!(!s.contains("SECRET1"));
    }

    #[test]
    fn identity_and_header() {
        let c = creds();
        assert!(c.has_session());
        assert_eq!(c.identity().language, Some("en"));
        assert_eq!(c.identity().region, None);
        assert!(c.cookie_header().contains("sessionid=SECRET1"));
        assert!(!Credentials::default().has_session());
    }

    #[test]
    fn accepts_camel_case_ids() {
        let c: Credentials =
            serde_json::from_str(r#"{"deviceId":"1","webId":"2","cookies":{"a":"b"}}"#).unwrap();
        assert_eq!((c.device_id.as_str(), c.web_id.as_str()), ("1", "2"));
    }
}
