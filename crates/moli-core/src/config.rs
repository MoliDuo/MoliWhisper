//! User settings, kept as typed JSON in the app data directory, readable
//! only by the user (mode 0600): the API keys live here too.
//!
//! Every field has a default, so a missing file, a missing key or a file
//! from an older version all load. A file that does not parse is moved
//! aside and the defaults are used.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hotkey::{Hotkey, Mode};

/// App data directory name; matches the bundle identifier.
pub const APP_ID: &str = "com.moliduo.moliwhisper";
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
    /// Rewrite the transcript as written text before pasting.
    pub organize: bool,
    pub qwen: QwenConfig,
    pub deepseek: DeepSeekConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            hotkey: Hotkey::default(),
            mode: Mode::default(),
            restore_clipboard: true,
            organize: false,
            qwen: QwenConfig::default(),
            deepseek: DeepSeekConfig::default(),
        }
    }
}

/// Qwen speech recognition on Alibaba Cloud.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QwenConfig {
    pub api_key: String,
}

/// DeepSeek, for the rewrite.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeepSeekConfig {
    pub api_key: String,
    /// System prompt; the built-in one when `None` or blank.
    pub prompt: Option<String>,
}

// Keys stay out of the logs.
impl std::fmt::Debug for QwenConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QwenConfig")
            .field("api_key", &redacted(&self.api_key))
            .finish()
    }
}

impl std::fmt::Debug for DeepSeekConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekConfig")
            .field("api_key", &redacted(&self.api_key))
            .field("prompt", &self.prompt)
            .finish()
    }
}

fn redacted(key: &str) -> &'static str {
    if key.is_empty() { "(none)" } else { "(set)" }
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

/// Write to a temp file in the same directory, then rename over the target.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        io::Write::write_all(&mut f, data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
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
    fn round_trip_and_permissions() {
        let dir = temp_dir("round");
        let store = ConfigStore::open(&dir);
        let c = Config {
            hotkey: Hotkey::Combo {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                code: 49,
            },
            mode: Mode::PushToTalk,
            restore_clipboard: false,
            organize: true,
            qwen: QwenConfig {
                api_key: "sk-q".into(),
            },
            deepseek: DeepSeekConfig {
                api_key: "sk-d".into(),
                prompt: Some("p".into()),
            },
            ..Config::default()
        };
        store.save(&c).unwrap();
        assert_eq!(store.load(), c);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
        assert!(!c.organize);
        assert_eq!(c.qwen, QwenConfig::default());
        assert_eq!(c.deepseek, DeepSeekConfig::default());
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

    #[test]
    fn debug_output_hides_the_keys() {
        let c = Config {
            qwen: QwenConfig {
                api_key: "SECRET-Q".into(),
            },
            deepseek: DeepSeekConfig {
                api_key: "SECRET-D".into(),
                prompt: None,
            },
            ..Config::default()
        };
        let shown = format!("{c:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
    }
}
