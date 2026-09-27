//! Query parameters of the ASR WebSocket URL.
//!
//! One constant table mirrors what the Doubao web client sends. Overrides from
//! the config are merged last, so a Doubao update that bumps a version number
//! can be fixed by editing the config instead of rebuilding the app.

use std::collections::BTreeMap;

use url::Url;

pub const ENDPOINT: &str = "wss://ws-samantha.doubao.com/samantha/audio/asr";
pub const ORIGIN: &str = "https://www.doubao.com";

/// Where each parameter's value comes from, in the order the web client sends them.
enum Value {
    Fixed(&'static str),
    Language,
    Region,
    DeviceId,
    WebId,
    WebTabId,
}

const TABLE: &[(&str, Value)] = &[
    ("version_code", Value::Fixed("20800")),
    ("language", Value::Language),
    ("device_platform", Value::Fixed("web")),
    ("doubao_device_platform", Value::Fixed("web")),
    ("aid", Value::Fixed("497858")),
    ("real_aid", Value::Fixed("497858")),
    ("pkg_type", Value::Fixed("release_version")),
    ("device_id", Value::DeviceId),
    ("pc_version", Value::Fixed("3.38.5")),
    ("doubao_pc_version", Value::Fixed("3.38.5")),
    ("web_id", Value::WebId),
    ("tea_uuid", Value::WebId),
    ("region", Value::Region),
    ("sys_region", Value::Region),
    ("samantha_web", Value::Fixed("1")),
    ("web_platform", Value::Fixed("browser")),
    ("use-olympus-account", Value::Fixed("1")),
    ("web_tab_id", Value::WebTabId),
    ("format", Value::Fixed("pcm")),
];

/// Per-account values that fill the dynamic slots of the table.
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    pub device_id: &'a str,
    pub web_id: &'a str,
    /// From the `i18next` cookie; `zh` when absent.
    pub language: Option<&'a str>,
    /// From the `flow_user_country` cookie; empty when absent.
    pub region: Option<&'a str>,
}

/// `Some(v)` sets or adds a parameter, `None` removes it.
pub type Overrides = BTreeMap<String, Option<String>>;

/// Builds the ordered parameter list. `web_tab_id` should be fresh per connection.
pub fn query(id: &Identity, web_tab_id: &str, overrides: &Overrides) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = TABLE
        .iter()
        .map(|(key, value)| {
            let v = match value {
                Value::Fixed(v) => v,
                Value::Language => id.language.filter(|l| !l.is_empty()).unwrap_or("zh"),
                Value::Region => id.region.unwrap_or(""),
                Value::DeviceId => id.device_id,
                Value::WebId => id.web_id,
                Value::WebTabId => web_tab_id,
            };
            (key.to_string(), v.to_string())
        })
        .collect();

    for (key, value) in overrides {
        let pos = out.iter().position(|(k, _)| k == key);
        match (pos, value) {
            (Some(i), Some(v)) => out[i].1 = v.clone(),
            (Some(i), None) => {
                out.remove(i);
            }
            (None, Some(v)) => out.push((key.clone(), v.clone())),
            (None, None) => {}
        }
    }
    out
}

pub fn url(id: &Identity, web_tab_id: &str, overrides: &Overrides) -> Url {
    let mut url = Url::parse(ENDPOINT).expect("valid endpoint");
    url.query_pairs_mut()
        .extend_pairs(query(id, web_tab_id, overrides));
    url
}

pub fn new_web_tab_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Identity = Identity {
        device_id: "111",
        web_id: "222",
        language: None,
        region: Some("CN"),
    };

    fn get<'a>(q: &'a [(String, String)], key: &str) -> Option<&'a str> {
        q.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn fills_dynamic_values() {
        let q = query(&ID, "tab", &Overrides::new());
        assert_eq!(q.len(), TABLE.len());
        assert_eq!(get(&q, "device_id"), Some("111"));
        assert_eq!(get(&q, "web_id"), Some("222"));
        assert_eq!(get(&q, "tea_uuid"), Some("222"));
        assert_eq!(get(&q, "web_tab_id"), Some("tab"));
        assert_eq!(get(&q, "language"), Some("zh"));
        assert_eq!(get(&q, "region"), Some("CN"));
        assert_eq!(get(&q, "sys_region"), Some("CN"));
        assert_eq!(get(&q, "format"), Some("pcm"));
        assert_eq!(q.last().unwrap().0, "format");
    }

    #[test]
    fn overrides_replace_remove_and_append() {
        let overrides = Overrides::from([
            ("pc_version".into(), Some("9.9.9".into())),
            ("web_platform".into(), None),
            ("extra".into(), Some("x".into())),
            ("absent".into(), None),
        ]);
        let q = query(&ID, "tab", &overrides);
        assert_eq!(get(&q, "pc_version"), Some("9.9.9"));
        assert_eq!(get(&q, "web_platform"), None);
        assert_eq!(q.last().unwrap(), &("extra".to_string(), "x".to_string()));
        assert_eq!(q.len(), TABLE.len());
    }

    #[test]
    fn url_encodes_values() {
        let overrides = Overrides::from([("extra".into(), Some("a b&c".into()))]);
        let u = url(&ID, "tab", &overrides);
        assert!(u.as_str().starts_with(ENDPOINT));
        assert!(u.query().unwrap().contains("extra=a+b%26c"));
        assert!(u.query().unwrap().contains("use-olympus-account=1"));
    }
}
