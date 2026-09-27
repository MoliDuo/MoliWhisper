//! Doubao login credentials: the `.doubao.com` cookies plus the two IDs the
//! web client keeps in localStorage.
//!
//! Cookie values are secrets. `Debug` redacts them, and nothing here logs them.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::asr::params::Identity;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// From localStorage `samantha_web_web_id`.web_id.
    #[serde(alias = "deviceId")]
    pub device_id: String,
    /// From localStorage `__tea_cache_tokens_497858`.web_id.
    #[serde(alias = "webId")]
    pub web_id: String,
    /// The cookies the browser sends to the ASR endpoint.
    pub cookies: BTreeMap<String, String>,
    /// From the `i18next` cookie of www.doubao.com, when it is not in `cookies`.
    #[serde(default)]
    pub language: Option<String>,
    /// From the `flow_user_country` cookie, when it is not in `cookies`.
    #[serde(default)]
    pub region: Option<String>,
}

impl Credentials {
    pub fn identity(&self) -> Identity<'_> {
        Identity {
            device_id: &self.device_id,
            web_id: &self.web_id,
            language: self
                .cookies
                .get("i18next")
                .or(self.language.as_ref())
                .map(String::as_str),
            region: self
                .cookies
                .get("flow_user_country")
                .or(self.region.as_ref())
                .map(String::as_str),
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

/// A cookie as a WebView's cookie store reports it.
#[derive(Debug, Clone, Copy)]
pub struct StoreCookie<'a> {
    pub name: &'a str,
    pub value: &'a str,
    /// With or without the leading dot.
    pub domain: &'a str,
    pub path: &'a str,
    pub secure: bool,
}

/// Picks the cookies a browser would send to `url` (RFC 6265 §5.4 domain and
/// path matching; the store has already dropped expired ones). When a name
/// appears more than once, the most specific path, then domain, wins.
pub fn cookies_for_url<'a>(
    url: &Url,
    cookies: impl IntoIterator<Item = StoreCookie<'a>>,
) -> BTreeMap<String, String> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path();
    let https = matches!(url.scheme(), "https" | "wss");

    let mut matching: Vec<StoreCookie> = cookies
        .into_iter()
        .filter(|c| {
            let domain = c.domain.trim_start_matches('.').to_ascii_lowercase();
            let domain_ok =
                !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")));
            domain_ok && path_matches(path, c.path) && (https || !c.secure)
        })
        .collect();
    matching
        .sort_by_key(|c| std::cmp::Reverse((c.path.len(), c.domain.trim_start_matches('.').len())));

    let mut out = BTreeMap::new();
    for c in matching {
        out.entry(c.name.to_string())
            .or_insert_with(|| c.value.to_string());
    }
    out
}

fn path_matches(request: &str, cookie: &str) -> bool {
    let cookie = if cookie.is_empty() { "/" } else { cookie };
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request[cookie.len()..].starts_with('/')))
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
            ..Default::default()
        }
    }

    #[test]
    fn picks_cookies_like_a_browser() {
        let c = |name, value, domain, path, secure| StoreCookie {
            name,
            value,
            domain,
            path,
            secure,
        };
        let url = Url::parse("wss://ws-samantha.doubao.com/samantha/audio/asr?x=1").unwrap();
        let got = cookies_for_url(
            &url,
            [
                c("sessionid", "s", ".doubao.com", "/", true),
                c("sid_guard", "g", "doubao.com", "/", false),
                c("host_only", "h", "www.doubao.com", "/", false),
                c(
                    "__moliwhisper_ids",
                    "m",
                    "www.doubao.com",
                    "/__moliwhisper",
                    false,
                ),
                c("pathy", "p", ".doubao.com", "/samantha", false),
                c("wrong_path", "w", ".doubao.com", "/sam", false),
                c("other", "o", ".example.com", "/", false),
                c("dup", "general", ".doubao.com", "/", false),
                c("dup", "specific", ".doubao.com", "/samantha/audio", false),
                c("evil", "e", "mantha.doubao.com", "/", false),
            ],
        );
        let names: Vec<_> = got.keys().map(String::as_str).collect();
        assert_eq!(names, ["dup", "pathy", "sessionid", "sid_guard"]);
        assert_eq!(got["dup"], "specific");

        let plain = Url::parse("ws://ws-samantha.doubao.com/").unwrap();
        let got = cookies_for_url(&plain, [c("sessionid", "s", ".doubao.com", "/", true)]);
        assert!(got.is_empty());
    }

    #[test]
    fn identity_falls_back_to_login_page_values() {
        let c = Credentials {
            language: Some("ja".into()),
            region: Some("JP".into()),
            ..creds()
        };
        assert_eq!(c.identity().language, Some("en"));
        assert_eq!(c.identity().region, Some("JP"));
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
