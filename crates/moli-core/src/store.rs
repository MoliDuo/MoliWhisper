//! Credential file: plain JSON in the app data directory, readable only by
//! the user (mode 0600 on Unix; the per-user profile ACL on Windows).

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::doubao::web::Credentials;

/// App data directory name; matches the bundle identifier.
pub const APP_ID: &str = "com.moliduo.moliwhisper";
const FILE_NAME: &str = "credentials.json";

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
    #[error("credential file is corrupt")]
    Corrupt,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct CredStore {
    path: PathBuf,
}

impl CredStore {
    /// Store in `dir` (the app data directory).
    pub fn open(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
        }
    }

    /// Store in the app's default data directory, for tools outside the app.
    pub fn open_default() -> Result<Self, StoreError> {
        let dir = dirs::data_dir()
            .ok_or_else(|| io::Error::other("no data directory"))?
            .join(APP_ID);
        Ok(Self::open(&dir))
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
        serde_json::from_slice(&data)
            .map(Some)
            .map_err(|_| StoreError::Corrupt)
    }

    pub fn save(&self, stored: &StoredCredentials) -> Result<(), StoreError> {
        let data = serde_json::to_vec_pretty(stored).map_err(io::Error::other)?;
        write_atomic(&self.path, &data)?;
        Ok(())
    }

    /// Removes the file. A missing file is not an error.
    pub fn clear(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Write to a temp file in the same directory, then rename over the target.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
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

    use super::*;

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
        let store = CredStore::open(&dir);
        assert!(store.load().unwrap().is_none());
        store.save(&sample()).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.credentials.cookies["sessionid"], "SECRET1");
        assert_eq!(loaded.rejected_at, None);

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        store.clear().unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn detects_a_corrupt_file() {
        let dir = temp_dir("bad");
        let store = CredStore::open(&dir);
        std::fs::write(store.path(), b"{not json").unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt)));
        std::fs::remove_dir_all(dir).ok();
    }
}
