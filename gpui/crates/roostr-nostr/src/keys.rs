//! The device's Nostr identity (keys.ts, Nostr.swift): secret parsing,
//! x-only public keys, BIP-340 signing and verification, ECDH, the author id.

use std::fmt;

use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::bech32;
use crate::event::{Event, EventTemplate, id_bytes};
use crate::random;

/// Why a key was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// Not `nsec1…` or 64 hex characters, or not a scalar in `[1, n-1]`.
    InvalidSecret,
    /// Not 64 hex characters naming a point on the curve.
    InvalidPubkey,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidSecret => "invalid secret key",
            Self::InvalidPubkey => "invalid public key",
        })
    }
}

impl std::error::Error for KeyError {}

/// A secp256k1 identity. `Debug` shows the public key only.
#[derive(Clone)]
pub struct Keys {
    /// The scalar as given (BIP-340 may negate it internally for signing).
    secret: [u8; 32],
    signing: SigningKey,
    pubkey: String,
}

impl PartialEq for Keys {
    fn eq(&self, other: &Self) -> bool {
        self.secret == other.secret
    }
}

impl Eq for Keys {}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keys")
            .field("pubkey", &self.pubkey)
            .finish_non_exhaustive()
    }
}

impl Keys {
    /// A 32-byte scalar in `[1, n-1]`.
    pub fn from_secret_bytes(secret: &[u8]) -> Result<Self, KeyError> {
        let secret: [u8; 32] = secret.try_into().map_err(|_| KeyError::InvalidSecret)?;
        let signing = SigningKey::from_bytes(&secret).map_err(|_| KeyError::InvalidSecret)?;
        let pubkey = hex::encode(signing.verifying_key().to_bytes());
        Ok(Self {
            secret,
            signing,
            pubkey,
        })
    }

    /// 64 hex characters, either case.
    pub fn from_secret_hex(text: &str) -> Result<Self, KeyError> {
        if text.len() != 64 {
            return Err(KeyError::InvalidSecret);
        }
        let bytes = hex::decode(text).map_err(|_| KeyError::InvalidSecret)?;
        Self::from_secret_bytes(&bytes)
    }

    /// keys.ts `parseKey`: an `nsec1…` string or a 64-hex secret,
    /// surrounding whitespace ignored.
    pub fn parse(input: &str) -> Result<Self, KeyError> {
        let trimmed = input.trim();
        if trimmed.len() == 64 && trimmed.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Self::from_secret_hex(trimmed);
        }
        if trimmed.starts_with("nsec1")
            && let Ok((hrp, data)) = bech32::decode(trimmed)
            && hrp == "nsec"
        {
            return Self::from_secret_bytes(&data);
        }
        Err(KeyError::InvalidSecret)
    }

    /// A fresh random identity (nostr-tools `generateSecretKey`).
    pub fn generate() -> Self {
        loop {
            // A random 32 bytes misses `[1, n-1]` with probability < 2^-127.
            if let Ok(keys) = Self::from_secret_bytes(&random::bytes::<32>()) {
                return keys;
            }
        }
    }

    pub fn secret_bytes(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Lowercase hex of the secret (what keys.ts persists).
    pub fn secret_hex(&self) -> String {
        hex::encode(self.secret)
    }

    /// x-only public key, lowercase hex.
    pub fn pubkey(&self) -> &str {
        &self.pubkey
    }

    pub fn npub(&self) -> String {
        bech32::encode("npub", &self.signing.verifying_key().to_bytes())
    }

    pub fn nsec(&self) -> String {
        bech32::encode("nsec", &self.secret)
    }

    /// [`author_id_for`] this secret.
    pub fn author_id(&self) -> String {
        author_id_for(&self.secret_hex())
    }

    /// nostr-tools `finalizeEvent`: the NIP-01 id, BIP-340 signed with fresh
    /// auxiliary randomness.
    pub fn sign(&self, template: EventTemplate) -> Event {
        self.sign_with_aux(template, &random::bytes::<32>())
    }

    /// [`Keys::sign`] with explicit BIP-340 auxiliary randomness: the same
    /// inputs give the same signature (tests, reproducible vectors).
    pub fn sign_with_aux(&self, template: EventTemplate, aux: &[u8; 32]) -> Event {
        let EventTemplate {
            kind,
            created_at,
            tags,
            content,
        } = template;
        let id = id_bytes(&self.pubkey, created_at, kind, &tags, &content);
        Event {
            id: hex::encode(id),
            pubkey: self.pubkey.clone(),
            created_at,
            kind,
            tags,
            content,
            sig: hex::encode(self.sign_hash_with_aux(&id, aux)),
        }
    }

    /// Raw BIP-340 signature of a 32-byte message (an event id, a digest).
    pub fn sign_hash(&self, hash: &[u8; 32]) -> [u8; 64] {
        self.sign_hash_with_aux(hash, &random::bytes::<32>())
    }

    /// [`Keys::sign_hash`] with explicit auxiliary randomness (BIP-340 test
    /// vectors, reproducible signatures).
    pub fn sign_hash_with_aux(&self, hash: &[u8; 32], aux: &[u8; 32]) -> [u8; 64] {
        // Fails only if a tagged hash lands on zero mod n: probability ~2^-256.
        let signature = self
            .signing
            .sign_raw(hash, aux)
            .expect("BIP-340 nonce derivation produced zero");
        signature.to_bytes()
    }

    /// The 32-byte x coordinate of `secret * pubkey` (ECDH): the input of a
    /// NIP-44 conversation key, and of the engine's `wire conversation_key`.
    pub fn shared_x(&self, pubkey_hex: &str) -> Result<[u8; 32], KeyError> {
        let theirs = lift_x(pubkey_hex)?;
        let secret = k256::NonZeroScalar::try_from(self.secret.as_slice())
            .map_err(|_| KeyError::InvalidSecret)?;
        let shared = k256::ecdh::diffie_hellman(secret, theirs.as_affine());
        Ok((*shared.raw_secret_bytes()).into())
    }
}

/// The even-y point with x coordinate `pubkey_hex` (BIP-340 `lift_x`).
fn lift_x(pubkey_hex: &str) -> Result<k256::PublicKey, KeyError> {
    let x = hex_32(pubkey_hex).ok_or(KeyError::InvalidPubkey)?;
    let mut sec1 = [0u8; 33];
    sec1[0] = 0x02;
    sec1[1..].copy_from_slice(&x);
    k256::PublicKey::from_sec1_bytes(&sec1).map_err(|_| KeyError::InvalidPubkey)
}

fn hex_32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    hex::decode(text).ok()?.try_into().ok()
}

/// BIP-340 verification of `sig_hex` over a 32-byte message by `pubkey_hex`.
pub fn verify_hash(pubkey_hex: &str, hash: &[u8; 32], sig_hex: &str) -> bool {
    let (Some(pubkey), Ok(sig)) = (hex_32(pubkey_hex), hex::decode(sig_hex)) else {
        return false;
    };
    let (Ok(key), Ok(sig)) = (
        VerifyingKey::from_bytes(&pubkey),
        Signature::try_from(sig.as_slice()),
    ) else {
        return false;
    };
    key.verify_raw(hash, &sig).is_ok()
}

/// The author id the Odin server derives (nostr.odin, keys.ts
/// `authorIdFor`): sha256 of the UTF-8 bytes of the lowercase 64-hex secret
/// *string*, first 8 bytes as 16 hex characters.
pub fn author_id_for(secret_hex: &str) -> String {
    let digest = Sha256::digest(secret_hex.to_ascii_lowercase().as_bytes());
    hex::encode(&digest[..8])
}

/// sync.ts `npubToHex`: an `npub1…` or 64-hex public key as lowercase hex.
pub fn npub_to_hex(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(hex) = bech32::decode_hex32(trimmed, "npub") {
        return Some(hex);
    }
    let lower = trimmed.to_ascii_lowercase();
    is_hex32(&lower).then_some(lower)
}

/// `npub1…` for a hex public key.
pub fn npub_from_hex(pubkey_hex: &str) -> Result<String, KeyError> {
    let bytes = hex_32(pubkey_hex).ok_or(KeyError::InvalidPubkey)?;
    Ok(bech32::encode("npub", &bytes))
}

/// 64 lowercase hex characters: an x-only pubkey, an event id, a space key.
pub fn is_hex32(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
