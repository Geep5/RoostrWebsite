//! The space keyring (spacekeys.ts) over [`LocalKv`] key `roostr-space-keys`,
//! the Svelte app's own key and file shape.

use std::collections::BTreeMap;
use std::sync::Arc;

use roostr_nostr::spacekeys::{KeyFile, KeyringError, SpaceKey, SpaceKeyring};

use crate::local::LocalKv;

pub const SPACE_KEYS: &str = "roostr-space-keys";

pub struct KvSpaceKeyring {
    kv: Arc<dyn LocalKv>,
}

impl KvSpaceKeyring {
    pub fn new(kv: Arc<dyn LocalKv>) -> Self {
        Self { kv }
    }

    fn read(&self) -> Result<KeyFile, KeyringError> {
        match self.kv.get(SPACE_KEYS) {
            None => Ok(KeyFile::default()),
            Some(text) if text == "null" => Ok(KeyFile::default()),
            Some(text) => KeyFile::parse(&text),
        }
    }

    fn write(&self, file: &KeyFile) {
        self.kv.set(SPACE_KEYS, Some(&file.to_json()));
    }
}

impl SpaceKeyring for KvSpaceKeyring {
    fn all(&self) -> Result<BTreeMap<String, SpaceKey>, KeyringError> {
        Ok(self.read()?.channels)
    }

    fn get(&self, space_id: &str) -> Result<Option<SpaceKey>, KeyringError> {
        Ok(self.read()?.channels.get(space_id).cloned())
    }

    fn set(&self, space_id: &str, key: SpaceKey) -> Result<(), KeyringError> {
        let mut file = self.read()?;
        file.channels.insert(space_id.to_string(), key);
        self.write(&file);
        Ok(())
    }

    fn remove(&self, space_id: &str) -> Result<(), KeyringError> {
        let mut file = self.read()?;
        if file.channels.remove(space_id).is_some() {
            self.write(&file);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::InMemoryLocalKv;

    #[test]
    fn reads_the_svelte_file_shape() {
        let kv = Arc::new(InMemoryLocalKv::new());
        let key = "ab".repeat(32);
        kv.set(
            SPACE_KEYS,
            Some(&format!(
                r#"{{"version":1,"channels":{{"s1":{{"key":"{key}","keyId":2,"createdAt":5,"owner":"{}"}}}}}}"#,
                "cd".repeat(32)
            )),
        );
        let ring = KvSpaceKeyring::new(kv.clone());
        let entry = ring.get("s1").unwrap().unwrap();
        assert_eq!((entry.key.as_str(), entry.key_id), (key.as_str(), 2));
        ring.ensure("s2").unwrap();
        ring.remove("s1").unwrap();
        let stored = kv.get(SPACE_KEYS).unwrap();
        assert!(
            stored.contains("\"version\":1")
                && stored.contains("\"s2\"")
                && !stored.contains("\"s1\"")
        );
    }
}
