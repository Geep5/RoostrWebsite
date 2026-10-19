//! The replica's identity (keys.ts): the secret key as 64 lowercase hex
//! under [`LocalKv`] key `roostr-key` - in the browser the Svelte app's own
//! `localStorage` entry, so a key saved on roostr.space is this app's key too.

use roostr_nostr::Keys;

use crate::local::LocalKv;

pub const KEY: &str = "roostr-key";

/// keys.ts `saveKey`'s message for input that is neither form.
pub const INVALID_KEY: &str = "invalid key: expected nsec1… or 64 hex chars";

/// The stored identity; `None` before key setup (or a stored value that no
/// longer parses, as keys.ts `loadKey`).
pub fn load(kv: &dyn LocalKv) -> Option<Keys> {
    Keys::parse(&kv.get(KEY)?).ok()
}

/// Parse `input` (`nsec1…` or 64 hex) and persist it.
pub fn save(kv: &dyn LocalKv, input: &str) -> anyhow::Result<Keys> {
    let keys = Keys::parse(input).map_err(|_| anyhow::anyhow!(INVALID_KEY))?;
    kv.set(KEY, Some(&keys.secret_hex()));
    Ok(keys)
}

/// A fresh identity, persisted (KeyGate's "Create account").
pub fn create(kv: &dyn LocalKv) -> Keys {
    let keys = Keys::generate();
    kv.set(KEY, Some(&keys.secret_hex()));
    keys
}

pub fn clear(kv: &dyn LocalKv) {
    kv.set(KEY, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::InMemoryLocalKv;

    #[test]
    fn stores_hex_like_keys_ts() {
        let kv = InMemoryLocalKv::new();
        assert!(load(&kv).is_none());
        let created = create(&kv);
        assert_eq!(kv.get(KEY).unwrap(), created.secret_hex());
        let nsec = created.nsec();
        clear(&kv);
        let saved = save(&kv, &format!("  {nsec}\n")).unwrap();
        assert_eq!(saved.pubkey(), created.pubkey());
        assert_eq!(load(&kv).unwrap().pubkey(), created.pubkey());
        assert_eq!(save(&kv, "nope").unwrap_err().to_string(), INVALID_KEY);
        // Upper-case hex is accepted and stored lower-case.
        let upper = created.secret_hex().to_uppercase();
        assert_eq!(save(&kv, &upper).unwrap().pubkey(), created.pubkey());
        assert_eq!(kv.get(KEY).unwrap(), created.secret_hex());
    }
}
