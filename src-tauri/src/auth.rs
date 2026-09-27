//! Login state: the stored credentials and what the tray shows about them.

use std::path::Path;
use std::sync::Mutex;

use moli_core::creds::Credentials;
use moli_core::store::{self, CredStore, StoreError, StoredCredentials};

/// Warn in the tray when the session has less than this left.
const EXPIRY_WARNING_SECS: u64 = 3 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    LoggedOut,
    Active {
        days_left: Option<u64>,
        expiring: bool,
    },
    /// The server refused the session; a new login is needed.
    Rejected,
    /// `sid_guard` says the session is past its lifetime.
    Expired,
}

pub struct Auth {
    store: Result<CredStore, String>,
    current: Mutex<Option<StoredCredentials>>,
}

impl Auth {
    pub fn load(data_dir: &Path) -> Self {
        let store = CredStore::open(data_dir).map_err(|e| e.to_string());
        let current = match &store {
            Ok(s) => match s.load() {
                Ok(c) => c,
                Err(e @ (StoreError::KeyMissing | StoreError::Corrupt)) => {
                    // Kept on disk; the next login overwrites it.
                    log::warn!("stored credentials unusable, treating as logged out: {e}");
                    None
                }
                Err(e) => {
                    log::error!("could not read stored credentials: {e}");
                    None
                }
            },
            Err(e) => {
                log::error!("credential store unavailable: {e}");
                None
            }
        };
        if let Some(c) = &current {
            log::info!("loaded credentials: {:?}", c.credentials);
        }
        Self {
            store,
            current: Mutex::new(current),
        }
    }

    pub fn status(&self) -> AuthStatus {
        let current = self.current.lock().unwrap();
        let Some(stored) = current.as_ref() else {
            return AuthStatus::LoggedOut;
        };
        if stored.rejected_at.is_some() {
            return AuthStatus::Rejected;
        }
        match stored.credentials.expires_at() {
            Some(at) => {
                let now = store::now();
                if at <= now {
                    AuthStatus::Expired
                } else {
                    let left = at - now;
                    AuthStatus::Active {
                        days_left: Some(left.div_ceil(86400)),
                        expiring: left < EXPIRY_WARNING_SECS,
                    }
                }
            }
            None => AuthStatus::Active {
                days_left: None,
                expiring: false,
            },
        }
    }

    /// The credentials to dictate with, if any.
    pub fn credentials(&self) -> Option<Credentials> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.credentials.clone())
    }

    pub fn save(&self, credentials: Credentials) -> Result<(), String> {
        let stored = StoredCredentials::new(credentials);
        self.store()?.save(&stored).map_err(|e| e.to_string())?;
        *self.current.lock().unwrap() = Some(stored);
        Ok(())
    }

    /// Marks the session refused by the server. The credentials stay.
    pub fn mark_rejected(&self) {
        let mut current = self.current.lock().unwrap();
        let Some(stored) = current.as_mut() else {
            return;
        };
        stored.rejected_at = Some(store::now());
        if let Ok(s) = &self.store
            && let Err(e) = s.save(stored)
        {
            log::error!("could not persist the rejected flag: {e}");
        }
    }

    pub fn logout(&self) -> Result<(), String> {
        *self.current.lock().unwrap() = None;
        self.store()?.clear().map_err(|e| e.to_string())
    }

    fn store(&self) -> Result<&CredStore, String> {
        self.store.as_ref().map_err(Clone::clone)
    }
}
