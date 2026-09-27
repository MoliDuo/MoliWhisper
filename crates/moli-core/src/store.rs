//! Encrypted credential file.
//!
//! A random 32-byte data key lives in the OS keychain (macOS Keychain, Windows
//! Credential Manager). The credentials JSON is sealed with XChaCha20-Poly1305
//! into `credentials.bin` in the app data directory. Keeping only the key in
//! the keychain sidesteps the 2560-byte limit of the Windows Credential Manager.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

use chacha20poly1305::aead::{Aead, Generate, Key, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::creds::Credentials;

/// Keychain service name and app data directory name; matches the bundle identifier.
pub const APP_ID: &str = "com.moliduo.moliwhisper";
const KEY_USER: &str = "credentials-key";
const FILE_NAME: &str = "credentials.bin";
/// File header, also bound into the ciphertext as associated data.
const MAGIC: &[u8; 4] = b"MWC1";
const NONCE_LEN: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub credentials: Credentials,
    /// Unix time of the login that produced these credentials.
    pub saved_at: u64,
    /// Unix time the server last rejected the session. The credentials are
    /// kept so the user can see what happened; a new login replaces them.
    #[serde(default)]
    pub rejected_at: Option<u64>,
}

impl StoredCredentials {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            saved_at: now(),
            rejected_at: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("keychain: {0}")]
    Keychain(String),
    /// The file exists but its key is gone (keychain reset, or another user).
    #[error("credential file exists but its key is missing from the keychain")]
    KeyMissing,
    #[error("credential file is corrupt or was sealed with another key")]
    Corrupt,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Where the data key is kept. The keychain in the app; memory in tests.
pub trait KeySource: Send + Sync {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError>;
    fn set(&self, key: &[u8]) -> Result<(), StoreError>;
    fn delete(&self) -> Result<(), StoreError>;
}

pub struct Keychain {
    entry: keyring_core::Entry,
}

impl Keychain {
    pub fn new() -> Result<Self, StoreError> {
        install_default_store()?;
        let entry = keyring_core::Entry::new(APP_ID, KEY_USER).map_err(keychain_err)?;
        Ok(Self { entry })
    }
}

impl KeySource for Keychain {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        match self.entry.get_secret() {
            Ok(key) => Ok(Some(key)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    fn set(&self, key: &[u8]) -> Result<(), StoreError> {
        self.entry.set_secret(key).map_err(keychain_err)
    }

    fn delete(&self) -> Result<(), StoreError> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

fn keychain_err(e: keyring_core::Error) -> StoreError {
    StoreError::Keychain(e.to_string())
}

fn install_default_store() -> Result<(), StoreError> {
    static ONCE: Once = Once::new();
    let mut result = Ok(());
    ONCE.call_once(|| {
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(not(any(target_os = "macos", windows)))]
        let store: Result<std::sync::Arc<keyring_core::CredentialStore>, _> = Err(
            keyring_core::Error::NotSupportedByStore("no keychain on this platform".into()),
        );
        match store {
            Ok(store) => keyring_core::set_default_store(store),
            Err(e) => result = Err(keychain_err(e)),
        }
    });
    result
}

pub struct CredStore<K = Keychain> {
    path: PathBuf,
    keys: K,
}

impl CredStore<Keychain> {
    /// Store in `dir` (the app data directory) with the key in the keychain.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        Ok(Self::with_keys(dir, Keychain::new()?))
    }

    /// Store in the app's default data directory, for tools outside the app.
    pub fn open_default() -> Result<Self, StoreError> {
        let dir = dirs::data_dir()
            .ok_or_else(|| io::Error::other("no data directory"))?
            .join(APP_ID);
        Self::open(&dir)
    }
}

impl<K: KeySource> CredStore<K> {
    pub fn with_keys(dir: &Path, keys: K) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            keys,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `Ok(None)` when nobody is logged in.
    pub fn load(&self) -> Result<Option<StoredCredentials>, StoreError> {
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let key = self.keys.get()?.ok_or(StoreError::KeyMissing)?;
        let plain = open(&key, &data)?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|_| StoreError::Corrupt)
    }

    pub fn save(&self, stored: &StoredCredentials) -> Result<(), StoreError> {
        let key = match self.keys.get()? {
            Some(k) if k.len() == 32 => k,
            _ => {
                let k = Key::<XChaCha20Poly1305>::generate().to_vec();
                self.keys.set(&k)?;
                k
            }
        };
        let plain = serde_json::to_vec(stored).map_err(io::Error::other)?;
        let sealed = seal(&key, &plain)?;
        write_atomic(&self.path, &sealed)?;
        Ok(())
    }

    /// Removes the file and the key. Missing pieces are not an error.
    pub fn clear(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.keys.delete()
    }
}

fn cipher(key: &[u8]) -> Result<XChaCha20Poly1305, StoreError> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| StoreError::Corrupt)
}

fn seal(key: &[u8], plain: &[u8]) -> Result<Vec<u8>, StoreError> {
    let nonce = XNonce::generate();
    let ct = cipher(key)?
        .encrypt(
            &nonce,
            Payload {
                msg: plain,
                aad: MAGIC,
            },
        )
        .map_err(|_| StoreError::Corrupt)?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn open(key: &[u8], data: &[u8]) -> Result<Vec<u8>, StoreError> {
    let rest = data
        .strip_prefix(MAGIC.as_slice())
        .ok_or(StoreError::Corrupt)?;
    if rest.len() < NONCE_LEN {
        return Err(StoreError::Corrupt);
    }
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| StoreError::Corrupt)?;
    cipher(key)?
        .decrypt(
            &nonce,
            Payload {
                msg: ct,
                aad: MAGIC,
            },
        )
        .map_err(|_| StoreError::Corrupt)
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

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemKeys(Mutex<Option<Vec<u8>>>);

    impl KeySource for MemKeys {
        fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn set(&self, key: &[u8]) -> Result<(), StoreError> {
            *self.0.lock().unwrap() = Some(key.to_vec());
            Ok(())
        }
        fn delete(&self) -> Result<(), StoreError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moli-store-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> StoredCredentials {
        StoredCredentials::new(Credentials {
            device_id: "d".into(),
            web_id: "w".into(),
            cookies: BTreeMap::from([("sessionid".into(), "SECRET1".into())]),
            ..Default::default()
        })
    }

    #[test]
    fn round_trip_and_clear() {
        let dir = temp_dir("rt");
        let store = CredStore::with_keys(&dir, MemKeys::default());
        assert!(store.load().unwrap().is_none());
        store.save(&sample()).unwrap();

        let raw = std::fs::read(store.path()).unwrap();
        assert!(raw.starts_with(MAGIC));
        assert!(!raw.windows(7).any(|w| w == b"SECRET1"));

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.credentials.cookies["sessionid"], "SECRET1");
        assert_eq!(loaded.rejected_at, None);

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(store.keys.get().unwrap().is_none());
        store.clear().unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn detects_missing_key_and_tampering() {
        let dir = temp_dir("bad");
        let store = CredStore::with_keys(&dir, MemKeys::default());
        store.save(&sample()).unwrap();

        let mut raw = std::fs::read(store.path()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        std::fs::write(store.path(), &raw).unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt)));

        store.keys.delete().unwrap();
        assert!(matches!(store.load(), Err(StoreError::KeyMissing)));
        std::fs::remove_dir_all(dir).ok();
    }
}
