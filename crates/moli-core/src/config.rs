//! User settings, kept as typed JSON next to the credentials.
//!
//! Every field has a default, so a missing file, a missing key or a file
//! from an older version all load. A file that does not parse is moved
//! aside and the defaults are used.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doubao::web::params::Overrides;
use crate::hotkey::{Hotkey, Mode};
use crate::store::write_atomic;

pub const FILE_NAME: &str = "config.json";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub hotkey: Hotkey,
    pub mode: Mode,
    /// Put the previous clipboard back after pasting.
    pub restore_clipboard: bool,
    /// Which recognition service to use.
    pub backend: BackendKind,
    /// Rewrite the transcript as written text before pasting.
    pub organize: bool,
    pub asr: AsrConfig,
    pub ime: ImeConfig,
    pub qwen: QwenConfig,
    /// Which service rewrites the transcript, when `organize` is on.
    pub organizer: OrganizerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            hotkey: Hotkey::default(),
            mode: Mode::default(),
            restore_clipboard: true,
            backend: BackendKind::default(),
            organize: false,
            asr: AsrConfig::default(),
            ime: ImeConfig::default(),
            qwen: QwenConfig::default(),
            organizer: OrganizerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// Merged over the built-in URL parameters; `null` removes one.
    pub param_overrides: Overrides,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The Doubao web ASR, with the login from the Doubao website.
    #[default]
    Web,
    /// The Doubao input method's ASR; anonymous, no login.
    Ime,
    /// Qwen speech recognition on Alibaba Cloud, with an API key.
    Qwen,
}

impl<'de> Deserialize<'de> for BackendKind {
    /// An unknown name (a backend from another version) falls back to the default.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(d)?.as_str() {
            "ime" => Self::Ime,
            "qwen" => Self::Qwen,
            _ => Self::Web,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImeConfig {
    /// The device id presented to the IME service, made on first use.
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QwenConfig {
    pub api_key: String,
    /// Blank means [`crate::qwen::DEFAULT_MODEL`].
    pub model: String,
    /// Blank means [`crate::qwen::DEFAULT_URL`].
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganizerConfig {
    pub provider: OrganizerKind,
    pub openai: OpenAiConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizerKind {
    /// The Doubao input method's rewrite; no setup.
    #[default]
    DoubaoIme,
    /// Any OpenAI-compatible chat completions API.
    Openai,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAiConfig {
    /// Up to and including the version, e.g. `https://api.deepseek.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// System prompt; the built-in one when `None` or blank.
    pub prompt: Option<String>,
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn open(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Never fails: problems are logged and the defaults used.
    pub fn load(&self) -> Config {
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Config::default(),
            Err(e) => {
                tracing::error!("could not read {:?}: {e}", self.path);
                return Config::default();
            }
        };
        match serde_json::from_slice::<Config>(&data) {
            Ok(mut c) => {
                c.version = VERSION;
                c
            }
            Err(e) => {
                let aside = self.path.with_extension("json.bad");
                tracing::warn!("{:?} is invalid ({e}); moved to {aside:?}", self.path);
                let _ = std::fs::rename(&self.path, &aside);
                Config::default()
            }
        }
    }

    pub fn save(&self, config: &Config) -> io::Result<()> {
        let data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
        write_atomic(&self.path, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{ModKey, Mods};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moli-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_gives_defaults() {
        let store = ConfigStore::open(&temp_dir("missing"));
        assert_eq!(store.load(), Config::default());
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("round");
        let store = ConfigStore::open(&dir);
        let mut c = Config {
            hotkey: Hotkey::Combo {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                code: 49,
            },
            mode: Mode::PushToTalk,
            restore_clipboard: false,
            backend: BackendKind::Ime,
            organize: true,
            ime: ImeConfig {
                device_id: Some("1234567890123456".into()),
            },
            qwen: QwenConfig {
                api_key: "sk-x".into(),
                model: "m".into(),
                url: "wss://example.com/ws".into(),
            },
            organizer: OrganizerConfig {
                provider: OrganizerKind::Openai,
                openai: OpenAiConfig {
                    base_url: "https://example.com/v1".into(),
                    api_key: "k".into(),
                    model: "m".into(),
                    prompt: Some("p".into()),
                },
            },
            ..Config::default()
        };
        c.asr
            .param_overrides
            .insert("pc_version".into(), Some("3.40.0".into()));
        c.asr.param_overrides.insert("region".into(), None);
        store.save(&c).unwrap();
        assert_eq!(store.load(), c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let dir = temp_dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), r#"{"mode":"push_to_talk"}"#).unwrap();
        let c = ConfigStore::open(&dir).load();
        assert_eq!(c.mode, Mode::PushToTalk);
        assert_eq!(
            c.hotkey,
            Hotkey::Modifier {
                key: ModKey::RightAlt
            }
        );
        assert!(c.restore_clipboard);
        assert_eq!(c.backend, BackendKind::Web);
        assert!(!c.organize);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_file_without_new_sections_loads() {
        let dir = temp_dir("old");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), r#"{"organize":true,"backend":"ime"}"#).unwrap();
        let c = ConfigStore::open(&dir).load();
        assert!(c.organize);
        assert_eq!(c.backend, BackendKind::Ime);
        assert_eq!(c.organizer.provider, OrganizerKind::DoubaoIme);
        assert_eq!(c.qwen, QwenConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), r#"{"backend":"self_hosted"}"#).unwrap();
        assert_eq!(ConfigStore::open(&dir).load().backend, BackendKind::Web);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_file_is_moved_aside() {
        let dir = temp_dir("invalid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "{not json").unwrap();
        let store = ConfigStore::open(&dir);
        assert_eq!(store.load(), Config::default());
        assert!(!dir.join(FILE_NAME).exists());
        assert!(dir.join("config.json.bad").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
