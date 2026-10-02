//! The user's settings: loaded once, saved on every change.

use std::path::Path;
use std::sync::Mutex;

use moli_core::config::{Config, ConfigStore};

pub struct Settings {
    store: ConfigStore,
    config: Mutex<Config>,
}

impl Settings {
    pub fn load(data_dir: &Path) -> Self {
        let store = ConfigStore::open(data_dir);
        let config = store.load();
        log::info!(
            "settings loaded: hotkey {:?}, mode {:?}, organize {}, qwen key {}, deepseek key {}",
            config.hotkey,
            config.mode,
            config.organize,
            !config.qwen.api_key.is_empty(),
            !config.deepseek.api_key.is_empty()
        );
        Self {
            store,
            config: Mutex::new(config),
        }
    }

    pub fn get(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    /// Changes and saves the settings; returns the new value.
    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Result<Config, String> {
        let mut config = self.config.lock().unwrap();
        let mut next = config.clone();
        f(&mut next);
        self.store
            .save(&next)
            .map_err(|e| format!("could not save {:?}: {e}", self.store.path()))?;
        *config = next.clone();
        Ok(next)
    }
}
