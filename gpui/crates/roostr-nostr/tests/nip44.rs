//! NIP-44 v2 against the spec's vectors (glonOdin/core/nip44_vectors.json,
//! the same file the engine and the iOS suite test against).

use roostr_nostr::Keys;
use roostr_nostr::nip44::{self, Nip44Error};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn vectors() -> Value {
    let path = format!(
        "{}/tests/fixtures/nip44_vectors.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    all["v2"].clone()
}

fn hex32(v: &Value) -> [u8; 32] {
    hex::decode(v.as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[test]
fn conversation_keys() {
    let v = vectors();
    for case in v["valid"]["get_conversation_key"].as_array().unwrap() {
        let keys = Keys::from_secret_hex(s(&case["sec1"])).unwrap();
        let key = nip44::conversation_key(&keys, s(&case["pub2"])).unwrap();
        assert_eq!(
            hex::encode(key),
            s(&case["conversation_key"]),
            "{}",
            s(&case["pub2"])
        );
        // Through the engine's input: the ECDH x coordinate.
        assert_eq!(
            nip44::conversation_key_from_shared_x(&keys.shared_x(s(&case["pub2"])).unwrap()),
            key
        );
    }
    for case in v["invalid"]["get_conversation_key"].as_array().unwrap() {
        let refused = Keys::from_secret_hex(s(&case["sec1"]))
            .map_err(|_| ())
            .and_then(|keys| nip44::conversation_key(&keys, s(&case["pub2"])).map_err(|_| ()));
        assert!(refused.is_err(), "{}", s(&case["note"]));
    }
}

#[test]
fn self_conversation_key_matches_the_ios_fixture() {
    let path = format!(
        "{}/tests/fixtures/nostr_fixtures.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fx: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let keys = Keys::from_secret_hex(s(&fx["secret"])).unwrap();
    assert_eq!(
        hex::encode(nip44::conversation_key(&keys, keys.pubkey()).unwrap()),
        s(&fx["selfConversationKey"])
    );
}

#[test]
fn message_keys() {
    let v = vectors();
    let ck = hex32(&v["valid"]["get_message_keys"]["conversation_key"]);
    for case in v["valid"]["get_message_keys"]["keys"].as_array().unwrap() {
        let keys = nip44::message_keys(&ck, &hex32(&case["nonce"]));
        assert_eq!(hex::encode(keys.chacha_key), s(&case["chacha_key"]));
        assert_eq!(hex::encode(keys.chacha_nonce), s(&case["chacha_nonce"]));
        assert_eq!(hex::encode(keys.hmac_key), s(&case["hmac_key"]));
    }
}

#[test]
fn padded_lengths() {
    for pair in vectors()["valid"]["calc_padded_len"].as_array().unwrap() {
        let (len, padded) = (
            pair[0].as_u64().unwrap() as usize,
            pair[1].as_u64().unwrap() as usize,
        );
        assert_eq!(nip44::padded_len(len), padded, "{len}");
    }
}

#[test]
fn encrypt_decrypt() {
    let v = vectors();
    for case in v["valid"]["encrypt_decrypt"].as_array().unwrap() {
        let sender = Keys::from_secret_hex(s(&case["sec1"])).unwrap();
        let recipient = Keys::from_secret_hex(s(&case["sec2"])).unwrap();
        let ck = nip44::conversation_key(&sender, recipient.pubkey()).unwrap();
        assert_eq!(hex::encode(ck), s(&case["conversation_key"]));
        assert_eq!(
            nip44::conversation_key(&recipient, sender.pubkey()).unwrap(),
            ck
        );
        let payload =
            nip44::encrypt_with_nonce(s(&case["plaintext"]), &ck, &hex32(&case["nonce"])).unwrap();
        assert_eq!(payload, s(&case["payload"]));
        assert_eq!(
            nip44::decrypt(&payload, &ck).unwrap(),
            s(&case["plaintext"])
        );
        // A fresh nonce still opens.
        assert_eq!(
            nip44::decrypt(&nip44::encrypt(s(&case["plaintext"]), &ck).unwrap(), &ck).unwrap(),
            s(&case["plaintext"])
        );
    }
}

#[test]
fn long_messages() {
    for case in vectors()["valid"]["encrypt_decrypt_long_msg"]
        .as_array()
        .unwrap()
    {
        let plaintext = s(&case["pattern"]).repeat(case["repeat"].as_u64().unwrap() as usize);
        assert_eq!(
            hex::encode(Sha256::digest(plaintext.as_bytes())),
            s(&case["plaintext_sha256"])
        );
        let ck = hex32(&case["conversation_key"]);
        let payload = nip44::encrypt_with_nonce(&plaintext, &ck, &hex32(&case["nonce"])).unwrap();
        assert_eq!(
            hex::encode(Sha256::digest(payload.as_bytes())),
            s(&case["payload_sha256"])
        );
        assert_eq!(nip44::decrypt(&payload, &ck).unwrap(), plaintext);
    }
}

#[test]
fn invalid_lengths_and_payloads() {
    let v = vectors();
    let ck = [7u8; 32];
    for len in v["invalid"]["encrypt_msg_lengths"].as_array().unwrap() {
        let plaintext = "a".repeat(len.as_u64().unwrap() as usize);
        assert_eq!(
            nip44::encrypt(&plaintext, &ck),
            Err(Nip44Error::PlaintextLength)
        );
    }
    for case in v["invalid"]["decrypt"].as_array().unwrap() {
        let result = nip44::decrypt(s(&case["payload"]), &hex32(&case["conversation_key"]));
        let note = s(&case["note"]);
        let expected = if note.starts_with("unknown encryption version") {
            Nip44Error::UnknownVersion
        } else if note == "invalid base64" {
            Nip44Error::Base64
        } else if note == "invalid MAC" {
            Nip44Error::Mac
        } else if note == "invalid padding" {
            Nip44Error::Padding
        } else {
            Nip44Error::PayloadSize
        };
        assert_eq!(result, Err(expected), "{note}");
    }
}
