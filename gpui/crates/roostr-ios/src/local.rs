//! The device-local values (`LocalKv`) where the Swift app kept them, so an
//! install that updates from it keeps its identity, space keys and relays:
//!
//! - `roostr-key`: the Keychain generic password `app.roostr.identity` /
//!   `nostr-secret` (the 32 secret bytes; `Identity.swift`).
//! - `roostr-space-keys`: the Keychain generic password
//!   `app.roostr.space-keys` / `channel-keys` (the key file's JSON;
//!   `SpaceKeyring.swift`).
//! - `roostr-relays`: `UserDefaults["roostr-relays"]`, a string array that
//!   survives logout (`RelaySettings`). Never configured: `ROOSTR_RELAYS`
//!   (`wss://a,wss://b`) when set, else the replica's default.
//! - everything else (profile cache, invites sent, join requests, …): a
//!   private JSON file beside the database.
//!
//! Keychain items are this-device-only and readable after the first unlock,
//! as the Swift app wrote them.

use std::collections::HashMap;
use std::path::Path;

use objc2_foundation::{NSArray, NSString, NSUserDefaults};
use parking_lot::Mutex;
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::{RELAYS, identity, keyring};
use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::passwords::{
    PasswordOptions, delete_generic_password_options, generic_password,
    set_generic_password_options,
};

/// `errSecItemNotFound`.
const ITEM_NOT_FOUND: i32 = -25300;

/// A Keychain generic-password item.
#[derive(Clone, Copy)]
struct Item {
    service: &'static str,
    account: &'static str,
}

const IDENTITY: Item = Item {
    service: "app.roostr.identity",
    account: "nostr-secret",
};

const SPACE_KEYS: Item = Item {
    service: "app.roostr.space-keys",
    account: "channel-keys",
};

impl Item {
    fn options(self) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(self.service, self.account);
        options.set_access_synchronized(Some(false));
        options
    }

    fn read(self) -> Option<Vec<u8>> {
        match generic_password(self.options()) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.code() == ITEM_NOT_FOUND => None,
            Err(error) => {
                log::error!("[keychain] read {}: {error}", self.service);
                None
            }
        }
    }

    fn write(self, bytes: &[u8]) {
        // An existing item keeps its accessibility; a new one is
        // this-device-only, readable after the first unlock.
        let result = if self.read().is_some() {
            set_generic_password_options(bytes, self.options())
        } else {
            SecAccessControl::create_with_protection(
                Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
                0,
            )
            .and_then(|access| {
                let mut options = self.options();
                options.set_access_control(access);
                set_generic_password_options(bytes, options)
            })
        };
        if let Err(error) = result {
            log::error!("[keychain] write {}: {error}", self.service);
        }
    }

    fn delete(self) {
        match delete_generic_password_options(self.options()) {
            Ok(()) => {}
            Err(error) if error.code() == ITEM_NOT_FOUND => {}
            Err(error) => log::error!("[keychain] delete {}: {error}", self.service),
        }
    }
}

pub struct IosLocalKv {
    file: FileLocalKv,
    /// What the Keychain holds, read once per key.
    keychain: Mutex<HashMap<&'static str, Option<String>>>,
}

impl IosLocalKv {
    /// The file part lives at `path` (mode 0600).
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            file: FileLocalKv::open(path)?,
            keychain: Mutex::new(HashMap::new()),
        })
    }

    fn keychain_get(&self, key: &'static str) -> Option<String> {
        let mut cache = self.keychain.lock();
        cache
            .entry(key)
            .or_insert_with(|| match key {
                identity::KEY => IDENTITY.read().map(hex::encode),
                _ => SPACE_KEYS
                    .read()
                    .and_then(|bytes| String::from_utf8(bytes).ok()),
            })
            .clone()
    }

    fn keychain_set(&self, key: &'static str, value: Option<&str>) {
        let item = if key == identity::KEY {
            IDENTITY
        } else {
            SPACE_KEYS
        };
        let stored = match value {
            None => {
                item.delete();
                None
            }
            Some(text) if key == identity::KEY => match hex::decode(text) {
                Ok(secret) if secret.len() == 32 => {
                    item.write(&secret);
                    Some(text.to_string())
                }
                _ => {
                    log::error!("[keychain] refusing a secret that is not 32 hex bytes");
                    return;
                }
            },
            Some(text) => {
                item.write(text.as_bytes());
                Some(text.to_string())
            }
        };
        self.keychain.lock().insert(key, stored);
    }
}

fn relays_default() -> Option<String> {
    let relays: Vec<String> = std::env::var("ROOSTR_RELAYS")
        .ok()?
        .split(',')
        .map(str::trim)
        .filter(|relay| !relay.is_empty())
        .map(String::from)
        .collect();
    serde_json::to_string(&relays).ok()
}

fn relays_get() -> Option<String> {
    let defaults = NSUserDefaults::standardUserDefaults();
    let Some(list) = defaults.stringArrayForKey(&NSString::from_str(RELAYS)) else {
        return relays_default();
    };
    let relays: Vec<String> = list.iter().map(|relay| relay.to_string()).collect();
    serde_json::to_string(&relays).ok()
}

fn relays_set(value: Option<&str>) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(RELAYS);
    let relays = value.and_then(|text| serde_json::from_str::<Option<Vec<String>>>(text).ok());
    match relays.flatten() {
        Some(relays) => {
            let strings: Vec<_> = relays
                .iter()
                .map(|relay| NSString::from_str(relay))
                .collect();
            let array = NSArray::from_retained_slice(&strings);
            // SAFETY: an NSArray of NSStrings is a property-list value.
            unsafe { defaults.setObject_forKey(Some(&array), &key) };
        }
        None => defaults.removeObjectForKey(&key),
    }
}

impl LocalKv for IosLocalKv {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            identity::KEY => self.keychain_get(identity::KEY),
            keyring::SPACE_KEYS => self.keychain_get(keyring::SPACE_KEYS),
            RELAYS => relays_get(),
            _ => self.file.get(key),
        }
    }

    fn set(&self, key: &str, value: Option<&str>) {
        match key {
            identity::KEY => self.keychain_set(identity::KEY, value),
            keyring::SPACE_KEYS => self.keychain_set(keyring::SPACE_KEYS, value),
            RELAYS => relays_set(value),
            _ => self.file.set(key, value),
        }
    }
}
