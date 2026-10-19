//! Space keys (spacekeys.ts, SpaceKeyring.swift, the daemon's
//! `channel-keys.json`), one file shape everywhere:
//! `{ version: 1, channels: { [spaceId]: { key, keyId, createdAt, owner? } } }`.
//!
//! `key` is 32 bytes of hex and doubles as the space's NIP-44 conversation
//! key; `owner` (hex pubkey) is set on imported entries and names who
//! administers the space. Owned spaces carry no owner: the local identity is.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::keys::is_hex32;
use crate::{random, time};

/// One space's current key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceKey {
    /// 64 lowercase hex.
    pub key: String,
    /// 1 for a new space, +1 per rotation.
    pub key_id: u64,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The administering identity's hex pubkey, on imported entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

impl SpaceKey {
    /// A fresh random key.
    pub fn random_key_hex() -> String {
        hex::encode(random::bytes::<32>())
    }
}

/// A storage failure, or a key file that is not version 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyringError(pub String);

impl fmt::Display for KeyringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space keyring: {}", self.0)
    }
}

impl std::error::Error for KeyringError {}

/// The persisted key file; a host stores [`KeyFile::to_json`] wherever it
/// keeps secrets (localStorage `roostr-space-keys`, the Keychain, a file).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyFile {
    pub channels: BTreeMap<String, SpaceKey>,
}

#[derive(Serialize, Deserialize)]
struct KeyFileWire {
    version: u32,
    channels: BTreeMap<String, SpaceKey>,
}

impl KeyFile {
    /// Parse a stored file. Anything but a version-1 file is an error, not
    /// an empty keyring: overwriting it would lose every key.
    pub fn parse(text: &str) -> Result<Self, KeyringError> {
        let wire: KeyFileWire = serde_json::from_str(text)
            .map_err(|e| KeyringError(format!("corrupt key file: {e}")))?;
        if wire.version != 1 {
            return Err(KeyringError(format!(
                "unsupported key file version {}",
                wire.version
            )));
        }
        Ok(Self {
            channels: wire.channels,
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&KeyFileWire {
            version: 1,
            channels: self.channels.clone(),
        })
        .expect("a key file always serializes")
    }
}

/// sync.ts/SpaceKeyring.swift's store: four primitives, the rules on top.
pub trait SpaceKeyring {
    fn all(&self) -> Result<BTreeMap<String, SpaceKey>, KeyringError>;
    fn get(&self, space_id: &str) -> Result<Option<SpaceKey>, KeyringError>;
    fn set(&self, space_id: &str, key: SpaceKey) -> Result<(), KeyringError>;
    /// Forget a space's key (this identity left a space it does not own).
    fn remove(&self, space_id: &str) -> Result<(), KeyringError>;

    /// Key for a locally created space (keyId 1). Idempotent.
    fn ensure(&self, space_id: &str) -> Result<SpaceKey, KeyringError> {
        if let Some(existing) = self.get(space_id)? {
            return Ok(existing);
        }
        let entry = SpaceKey {
            key: SpaceKey::random_key_hex(),
            key_id: 1,
            created_at: time::now_ms(),
            owner: None,
        };
        self.set(space_id, entry.clone())?;
        Ok(entry)
    }

    /// Rotate on member removal: a new key, keyId + 1; an imported owner is
    /// kept.
    fn rotate(&self, space_id: &str) -> Result<SpaceKey, KeyringError> {
        let previous = self.get(space_id)?;
        let entry = SpaceKey {
            key: SpaceKey::random_key_hex(),
            key_id: previous.as_ref().map_or(0, |p| p.key_id) + 1,
            created_at: time::now_ms(),
            owner: previous.and_then(|p| p.owner).filter(|o| !o.is_empty()),
        };
        self.set(space_id, entry.clone())?;
        Ok(entry)
    }

    /// Import a key someone else administers; a local entry with the same
    /// or a newer keyId wins (spacekeys.ts `spaceKeyImport`).
    fn import(
        &self,
        space_id: &str,
        key: &str,
        key_id: u64,
        owner: &str,
    ) -> Result<SpaceKey, KeyringError> {
        if let Some(previous) = self.get(space_id)?
            && previous.key_id >= key_id
        {
            return Ok(previous);
        }
        let entry = SpaceKey {
            key: key.to_string(),
            key_id,
            created_at: time::now_ms(),
            owner: Some(owner.to_string()),
        };
        self.set(space_id, entry.clone())?;
        Ok(entry)
    }

    /// The gate a delivered key passes before [`SpaceKeyring::import`]
    /// (sync.ts `importSpaceInvite` and `handleWrap`): a well-formed key and
    /// keyId, and, for a space already held, only its administrator (the
    /// recorded owner, or this identity `local_pk` for an owned space) may
    /// replace the key. `Ok(false)` when refused.
    fn accept_invite(
        &self,
        space_id: &str,
        owner: &str,
        key: &str,
        key_id: u64,
        local_pk: &str,
    ) -> Result<bool, KeyringError> {
        if space_id.is_empty() || !is_hex32(key) || !is_hex32(owner) || key_id < 1 {
            return Ok(false);
        }
        if let Some(previous) = self.get(space_id)? {
            let administrator = previous
                .owner
                .as_deref()
                .filter(|o| !o.is_empty())
                .unwrap_or(local_pk);
            if administrator != owner {
                return Ok(false);
            }
        }
        self.import(space_id, key, key_id, owner)?;
        Ok(true)
    }
}

/// Who administers a space, as `SpaceJSON.owner` reports it: `""` when this
/// identity does (a space created here, or an import naming our own key),
/// otherwise the owner's hex pubkey.
pub fn space_owner(entry: Option<&SpaceKey>, local_pk: &str) -> String {
    let owner = entry.and_then(|e| e.owner.as_deref()).unwrap_or("");
    if owner == local_pk {
        String::new()
    } else {
        owner.to_string()
    }
}

/// A keyring held in memory (tests, hosts that persist [`KeyFile`] around it).
#[derive(Default)]
pub struct InMemorySpaceKeyring {
    file: parking_lot::Mutex<KeyFile>,
}

impl InMemorySpaceKeyring {
    pub fn new(file: KeyFile) -> Self {
        Self {
            file: parking_lot::Mutex::new(file),
        }
    }

    /// The current contents, to persist.
    pub fn snapshot(&self) -> KeyFile {
        self.lock().clone()
    }

    fn lock(&self) -> parking_lot::MutexGuard<'_, KeyFile> {
        self.file.lock()
    }
}

impl SpaceKeyring for InMemorySpaceKeyring {
    fn all(&self) -> Result<BTreeMap<String, SpaceKey>, KeyringError> {
        Ok(self.lock().channels.clone())
    }

    fn get(&self, space_id: &str) -> Result<Option<SpaceKey>, KeyringError> {
        Ok(self.lock().channels.get(space_id).cloned())
    }

    fn set(&self, space_id: &str, key: SpaceKey) -> Result<(), KeyringError> {
        self.lock().channels.insert(space_id.to_string(), key);
        Ok(())
    }

    fn remove(&self, space_id: &str) -> Result<(), KeyringError> {
        self.lock().channels.remove(space_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const OWNER: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const KEY_A: &str = "abababababababababababababababababababababababababababababababab";
    const KEY_B: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    #[test]
    fn reads_the_desktop_and_web_file_shape() {
        let text = r#"{"version":1,"channels":{"s1":{"key":"abababababababababababababababababababababababababababababababab","keyId":2,"createdAt":1700000000000,"owner":"2222222222222222222222222222222222222222222222222222222222222222"},"s2":{"key":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","keyId":1,"createdAt":1}}}"#;
        let file = KeyFile::parse(text).unwrap();
        assert_eq!(file.channels["s1"].owner.as_deref(), Some(OWNER));
        assert_eq!(file.channels["s2"].owner, None);
        // Owned entries carry no `owner` key at all, like spacekeys.ts writes them.
        assert_eq!(file.to_json(), text);
        assert!(KeyFile::parse(r#"{"version":2,"channels":{}}"#).is_err());
        assert!(KeyFile::parse("not json").is_err());
    }

    #[test]
    fn ensure_rotate_import_follow_spacekeys_ts() {
        let ring = InMemorySpaceKeyring::default();
        let made = ring.ensure("s").unwrap();
        assert_eq!((made.key_id, made.owner.as_deref()), (1, None));
        assert!(is_hex32(&made.key));
        assert_eq!(ring.ensure("s").unwrap(), made);
        let rotated = ring.rotate("s").unwrap();
        assert_eq!(rotated.key_id, 2);
        assert_ne!(rotated.key, made.key);

        // An older or equal keyId never replaces the local key.
        assert_eq!(ring.import("s", KEY_A, 2, OWNER).unwrap(), rotated);
        let imported = ring.import("s", KEY_A, 3, OWNER).unwrap();
        assert_eq!(
            (
                imported.key.as_str(),
                imported.key_id,
                imported.owner.as_deref()
            ),
            (KEY_A, 3, Some(OWNER))
        );
        // A rotation of an imported space keeps its owner.
        assert_eq!(ring.rotate("s").unwrap().owner.as_deref(), Some(OWNER));
    }

    #[test]
    fn only_the_administrator_may_replace_a_key() {
        let ring = InMemorySpaceKeyring::default();
        assert!(ring.accept_invite("joined", OWNER, KEY_A, 1, ME).unwrap());
        // Someone else claiming the space is refused; the owner's rotation is not.
        assert!(!ring.accept_invite("joined", ME, KEY_B, 2, ME).unwrap());
        assert!(ring.accept_invite("joined", OWNER, KEY_B, 2, ME).unwrap());
        assert_eq!(ring.get("joined").unwrap().unwrap().key, KEY_B);

        // A space created here is only re-keyed by this identity.
        ring.ensure("mine").unwrap();
        assert!(!ring.accept_invite("mine", OWNER, KEY_A, 5, ME).unwrap());
        assert!(ring.accept_invite("mine", ME, KEY_A, 5, ME).unwrap());

        // Malformed deliveries.
        assert!(!ring.accept_invite("", OWNER, KEY_A, 1, ME).unwrap());
        assert!(!ring.accept_invite("x", OWNER, "ABAB", 1, ME).unwrap());
        assert!(!ring.accept_invite("x", OWNER, KEY_A, 0, ME).unwrap());
        assert!(!ring.accept_invite("x", "npub1…", KEY_A, 1, ME).unwrap());
    }

    #[test]
    fn space_owner_hides_the_local_identity() {
        let owned = SpaceKey {
            key: KEY_A.into(),
            key_id: 1,
            created_at: 0,
            owner: None,
        };
        let imported = SpaceKey {
            owner: Some(OWNER.into()),
            ..owned.clone()
        };
        let mine = SpaceKey {
            owner: Some(ME.into()),
            ..owned.clone()
        };
        assert_eq!(space_owner(Some(&owned), ME), "");
        assert_eq!(space_owner(Some(&mine), ME), "");
        assert_eq!(space_owner(Some(&imported), ME), OWNER);
        assert_eq!(space_owner(None, ME), "");
    }
}
