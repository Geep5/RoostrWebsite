//! NIP-44 v2 payloads, the same algorithm as the engine's `core/nip44.odin`
//! and nostr-tools' `nip44`, held to the spec's test vectors. The host uses
//! it for gift wraps (NIP-59); the change stream is sealed by the engine.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::keys::{KeyError, Keys};
use crate::random;

const VERSION: u8 = 2;
const MIN_PLAINTEXT: usize = 1;
const MAX_PLAINTEXT: usize = 65535;
const SALT: &[u8] = b"nip44-v2";

/// Why a payload could not be made or opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nip44Error {
    /// Plaintext shorter than 1 or longer than 65535 bytes.
    PlaintextLength,
    /// A version other than 2 (including the reserved `#` prefix).
    UnknownVersion,
    /// The base64 text or the decoded payload has an impossible size.
    PayloadSize,
    /// Not standard padded base64.
    Base64,
    /// The MAC does not authenticate the ciphertext under this key.
    Mac,
    /// The decrypted length prefix or padding is inconsistent.
    Padding,
    /// The plaintext is not UTF-8.
    Utf8,
}

impl fmt::Display for Nip44Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PlaintextLength => "nip44: invalid plaintext length",
            Self::UnknownVersion => "nip44: unknown encryption version",
            Self::PayloadSize => "nip44: invalid payload size",
            Self::Base64 => "nip44: invalid base64",
            Self::Mac => "nip44: invalid MAC",
            Self::Padding => "nip44: invalid padding",
            Self::Utf8 => "nip44: plaintext is not UTF-8",
        })
    }
}

impl std::error::Error for Nip44Error {}

/// The per-message keys derived from a conversation key and a nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageKeys {
    pub chacha_key: [u8; 32],
    pub chacha_nonce: [u8; 12],
    pub hmac_key: [u8; 32],
}

/// HKDF-extract of an ECDH shared x coordinate with the spec salt (the
/// engine's `wire conversation_key`).
pub fn conversation_key_from_shared_x(shared_x: &[u8; 32]) -> [u8; 32] {
    let (prk, _) = Hkdf::<Sha256>::extract(Some(SALT), shared_x);
    prk.into()
}

/// The conversation key between `keys` and the hex public key `pubkey_hex`;
/// symmetric, so both sides derive the same one.
pub fn conversation_key(keys: &Keys, pubkey_hex: &str) -> Result<[u8; 32], KeyError> {
    Ok(conversation_key_from_shared_x(&keys.shared_x(pubkey_hex)?))
}

/// HKDF-expand of the conversation key with the nonce as info.
pub fn message_keys(conversation_key: &[u8; 32], nonce: &[u8; 32]) -> MessageKeys {
    let hkdf = Hkdf::<Sha256>::from_prk(conversation_key).expect("a 32-byte PRK is valid");
    let mut okm = [0u8; 76];
    hkdf.expand(nonce, &mut okm)
        .expect("76 bytes is a valid HKDF length");
    let mut keys = MessageKeys {
        chacha_key: [0; 32],
        chacha_nonce: [0; 12],
        hmac_key: [0; 32],
    };
    keys.chacha_key.copy_from_slice(&okm[..32]);
    keys.chacha_nonce.copy_from_slice(&okm[32..44]);
    keys.hmac_key.copy_from_slice(&okm[44..]);
    keys
}

/// Padded size of an `unpadded`-byte plaintext (spec `calc_padded_len`).
pub fn padded_len(unpadded: usize) -> usize {
    if unpadded <= 32 {
        return 32;
    }
    let next_power = 1usize << (usize::BITS - (unpadded - 1).leading_zeros());
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    chunk * ((unpadded - 1) / chunk + 1)
}

/// Encrypt under a fresh random nonce; returns the base64 payload.
pub fn encrypt(plaintext: &str, conversation_key: &[u8; 32]) -> Result<String, Nip44Error> {
    encrypt_with_nonce(plaintext, conversation_key, &random::bytes::<32>())
}

/// Encrypt with an explicit nonce (test vectors only: a reused nonce leaks
/// the plaintext).
pub fn encrypt_with_nonce(
    plaintext: &str,
    conversation_key: &[u8; 32],
    nonce: &[u8; 32],
) -> Result<String, Nip44Error> {
    let len = plaintext.len();
    if !(MIN_PLAINTEXT..=MAX_PLAINTEXT).contains(&len) {
        return Err(Nip44Error::PlaintextLength);
    }
    let keys = message_keys(conversation_key, nonce);
    let mut padded = vec![0u8; 2 + padded_len(len)];
    padded[..2].copy_from_slice(&(len as u16).to_be_bytes());
    padded[2..2 + len].copy_from_slice(plaintext.as_bytes());
    ChaCha20::new(&keys.chacha_key.into(), &keys.chacha_nonce.into()).apply_keystream(&mut padded);
    let mac = mac(&keys.hmac_key, nonce, &padded);
    let mut payload = Vec::with_capacity(1 + 32 + padded.len() + 32);
    payload.push(VERSION);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(&padded);
    payload.extend_from_slice(&mac);
    Ok(BASE64.encode(payload))
}

/// Open a base64 payload.
pub fn decrypt(payload: &str, conversation_key: &[u8; 32]) -> Result<String, Nip44Error> {
    // nostr-tools' order: the text length, the reserved `#`, then the bytes.
    if !(132..=87472).contains(&payload.len()) {
        return Err(Nip44Error::PayloadSize);
    }
    if payload.starts_with('#') {
        return Err(Nip44Error::UnknownVersion);
    }
    let data = BASE64.decode(payload).map_err(|_| Nip44Error::Base64)?;
    if !(99..=65603).contains(&data.len()) {
        return Err(Nip44Error::PayloadSize);
    }
    if data[0] != VERSION {
        return Err(Nip44Error::UnknownVersion);
    }
    let nonce: [u8; 32] = data[1..33].try_into().expect("checked length");
    let (ciphertext, tag) = data[33..].split_at(data.len() - 33 - 32);
    let keys = message_keys(conversation_key, &nonce);
    let mut verifier = <Hmac<Sha256> as Mac>::new_from_slice(&keys.hmac_key).expect("any key size");
    verifier.update(&nonce);
    verifier.update(ciphertext);
    verifier.verify_slice(tag).map_err(|_| Nip44Error::Mac)?;
    let mut padded = ciphertext.to_vec();
    ChaCha20::new(&keys.chacha_key.into(), &keys.chacha_nonce.into()).apply_keystream(&mut padded);
    let len = usize::from(u16::from_be_bytes([padded[0], padded[1]]));
    if len < MIN_PLAINTEXT || padded.len() != 2 + padded_len(len) {
        return Err(Nip44Error::Padding);
    }
    padded.truncate(2 + len);
    padded.drain(..2);
    String::from_utf8(padded).map_err(|_| Nip44Error::Utf8)
}

fn mac(key: &[u8; 32], nonce: &[u8; 32], ciphertext: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key size");
    mac.update(nonce);
    mac.update(ciphertext);
    mac.finalize().into_bytes().into()
}
