//! Device-local small values: what the Svelte replica keeps in
//! `localStorage` (`roostr-key`, `roostr-space-keys`, `roostr-relays`,
//! `roostr-invites-sent`, join requests, …), under the same key names.
//!
//! In the browser it *is* `localStorage` - the GPUI build is served from the
//! same origin as the Svelte app, so a key saved by roostr.space/app is
//! picked up here. Natively it is one JSON object in a file readable only by
//! the user (mode 0600), since it holds the secret key and the space keys.

#[cfg(not(target_family = "wasm"))]
use std::collections::BTreeMap;
use std::collections::HashMap;

use parking_lot::Mutex;

use crate::{MaybeSend, MaybeSync};

pub trait LocalKv: MaybeSend + MaybeSync {
    fn get(&self, key: &str) -> Option<String>;
    /// `None` removes the key.
    fn set(&self, key: &str, value: Option<&str>);
}

/// Values kept in memory only (tests, a throwaway replica).
#[derive(Default)]
pub struct InMemoryLocalKv {
    values: Mutex<HashMap<String, String>>,
}

impl InMemoryLocalKv {
    pub fn new() -> Self {
        Self::default()
    }
}

impl LocalKv for InMemoryLocalKv {
    fn get(&self, key: &str) -> Option<String> {
        self.values.lock().get(key).cloned()
    }

    fn set(&self, key: &str, value: Option<&str>) {
        let mut values = self.values.lock();
        match value {
            Some(value) => values.insert(key.to_string(), value.to_string()),
            None => values.remove(key),
        };
    }
}

/// The page's `localStorage`. Unavailable storage (a sandboxed frame)
/// reads as empty and drops writes, as keys.ts' guards do.
#[cfg(target_family = "wasm")]
#[derive(Default)]
pub struct BrowserLocalKv;

#[cfg(target_family = "wasm")]
impl BrowserLocalKv {
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
}

#[cfg(target_family = "wasm")]
impl LocalKv for BrowserLocalKv {
    fn get(&self, key: &str) -> Option<String> {
        Self::storage()?.get_item(key).ok().flatten()
    }

    fn set(&self, key: &str, value: Option<&str>) {
        let Some(storage) = Self::storage() else {
            return;
        };
        let result = match value {
            Some(value) => storage.set_item(key, value),
            None => storage.remove_item(key),
        };
        if result.is_err() {
            log::warn!("[replica] localStorage refused {key}");
        }
    }
}

/// A JSON object file (`{"roostr-key": "…", …}`), rewritten atomically with
/// mode 0600 on every change.
#[cfg(not(target_family = "wasm"))]
pub struct FileLocalKv {
    path: std::path::PathBuf,
    values: Mutex<BTreeMap<String, String>>,
}

#[cfg(not(target_family = "wasm"))]
impl FileLocalKv {
    /// Opens (or starts) the file at `path`. A file that does not parse is
    /// an error, not an empty store: overwriting it would lose the key.
    pub fn open(path: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let values = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                anyhow::anyhow!("{} is not a key/value file: {error}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
        };
        Ok(Self {
            path,
            values: Mutex::new(values),
        })
    }

    fn write(&self, values: &BTreeMap<String, String>) -> std::io::Result<()> {
        use std::io::Write as _;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt as _;

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = self.path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temp)?;
        file.write_all(serde_json::to_string_pretty(values)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &self.path)
    }
}

#[cfg(not(target_family = "wasm"))]
impl LocalKv for FileLocalKv {
    fn get(&self, key: &str) -> Option<String> {
        self.values.lock().get(key).cloned()
    }

    fn set(&self, key: &str, value: Option<&str>) {
        let mut values = self.values.lock();
        let changed = match value {
            Some(value) => {
                values.insert(key.to_string(), value.to_string()).as_deref() != Some(value)
            }
            None => values.remove(key).is_some(),
        };
        if changed && let Err(error) = self.write(&values) {
            log::error!("[replica] cannot write {}: {error}", self.path.display());
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn file_kv_round_trips_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/local.json");
        let kv = FileLocalKv::open(&path).unwrap();
        kv.set("roostr-key", Some("abc"));
        kv.set("gone", Some("x"));
        kv.set("gone", None);
        let again = FileLocalKv::open(&path).unwrap();
        assert_eq!(again.get("roostr-key").as_deref(), Some("abc"));
        assert_eq!(again.get("gone"), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(FileLocalKv::open(&path).is_err());
    }
}
