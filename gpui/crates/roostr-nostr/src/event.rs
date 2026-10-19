//! NIP-01 events: the canonical id serialization, exactly as nostr-tools'
//! `getEventHash` (`JSON.stringify` of `[0,pubkey,created_at,kind,tags,content]`)
//! and the iOS host's `NostrEvent.computeId` produce it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::keys::verify_hash;

/// A signed Nostr event. Serializes with nostr-tools' field names
/// (`id, pubkey, created_at, kind, tags, content, sig`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Event {
    /// Lowercase hex sha256 of the canonical serialization.
    pub id: String,
    /// Author's x-only public key, lowercase hex.
    pub pubkey: String,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    /// BIP-340 signature over the id, lowercase hex.
    pub sig: String,
}

/// What a signer turns into an [`Event`] (nostr-tools' `EventTemplate`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTemplate {
    pub kind: u32,
    pub created_at: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
}

impl Event {
    /// The id matches the fields and the signature verifies against `pubkey`
    /// (nostr-tools `verifyEvent`).
    pub fn verify(&self) -> bool {
        let Ok(id) = <[u8; 32]>::try_from(hex_lower(&self.id).as_slice()) else {
            return false;
        };
        id == id_bytes(
            &self.pubkey,
            self.created_at,
            self.kind,
            &self.tags,
            &self.content,
        ) && verify_hash(&self.pubkey, &id, &self.sig)
    }

    /// The first value of the first tag named `name` (`["p", <value>]`).
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tag_values(name).next()
    }

    /// The first value of every tag named `name`, in order.
    pub fn tag_values<'a, 'n>(
        &'a self,
        name: &'n str,
    ) -> impl Iterator<Item = &'a str> + use<'a, 'n> {
        self.tags
            .iter()
            .filter(move |t| t.first().is_some_and(|n| n == name))
            .filter_map(|t| t.get(1).map(String::as_str))
    }

    /// JSON text (`JSON.stringify(event)`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an event always serializes")
    }
}

/// Strict lowercase hex; empty on anything else, so a malformed id never
/// compares equal.
fn hex_lower(text: &str) -> Vec<u8> {
    if text.bytes().any(|c| c.is_ascii_uppercase()) {
        return Vec::new();
    }
    hex::decode(text).unwrap_or_default()
}

/// NIP-01 event id, lowercase hex.
pub fn event_id(
    pubkey: &str,
    created_at: u64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    hex::encode(id_bytes(pubkey, created_at, kind, tags, content))
}

pub(crate) fn id_bytes(
    pubkey: &str,
    created_at: u64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> [u8; 32] {
    let mut out = Vec::with_capacity(96 + content.len() + tags.len() * 32);
    out.extend_from_slice(b"[0,");
    push_string(&mut out, pubkey);
    out.push(b',');
    out.extend_from_slice(created_at.to_string().as_bytes());
    out.push(b',');
    out.extend_from_slice(kind.to_string().as_bytes());
    out.extend_from_slice(b",[");
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'[');
        for (j, value) in tag.iter().enumerate() {
            if j > 0 {
                out.push(b',');
            }
            push_string(&mut out, value);
        }
        out.push(b']');
    }
    out.extend_from_slice(b"],");
    push_string(&mut out, content);
    out.push(b']');
    Sha256::digest(&out).into()
}

/// A JSON string as `JSON.stringify` writes it: only `"`, `\` and C0 controls
/// are escaped (the short forms where JSON has one), everything else,
/// DEL and non-ASCII included, is verbatim UTF-8.
fn push_string(out: &mut Vec<u8>, value: &str) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for &byte in value.as_bytes() {
        match byte {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            0..0x20 => {
                out.extend_from_slice(b"\\u00");
                out.push(DIGITS[usize::from(byte >> 4)]);
                out.push(DIGITS[usize::from(byte & 0x0f)]);
            }
            _ => out.push(byte),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_json_stringify() {
        // \u0001 and \u001f become \u00XX; DEL and non-ASCII stay raw.
        let pubkey = "142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9";
        let canonical = format!("[0,\"{pubkey}\",1,1,[],\"a\\u0001b\\u001fc\u{7f}é\"]");
        assert_eq!(
            event_id(pubkey, 1, 1, &[], "a\u{01}b\u{1f}c\u{7f}é"),
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );
        // serde_json escapes the same set the same way.
        let content = "q\" b\\ / \n\r\t\u{8}\u{c}\u{0}\u{1f}\u{7f}ä🎉\u{2028}";
        let mut ours = Vec::new();
        push_string(&mut ours, content);
        assert_eq!(
            String::from_utf8(ours).unwrap(),
            serde_json::to_string(content).unwrap()
        );
    }
}
