//! Keys, ids and signatures against what the other hosts produce:
//! nostr-tools (`nostr_tools_vectors.json`, recorded from the retired Svelte
//! site's nostr-tools 2.x, and the iOS suite's `nostr_fixtures.json`) and the
//! BIP-340 reference vectors.

use roostr_nostr::keys::{author_id_for, is_hex32, npub_from_hex, npub_to_hex, verify_hash};
use roostr_nostr::{Event, EventTemplate, KeyError, Keys, bech32, event_id};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn hex32(text: &str) -> [u8; 32] {
    hex::decode(text).unwrap().try_into().unwrap()
}

#[test]
fn nostr_tools_keys_and_bech32() {
    let vectors = fixture("nostr_tools_vectors.json");
    for key in vectors["keys"].as_array().unwrap() {
        let keys = Keys::from_secret_hex(s(&key["secret"])).unwrap();
        assert_eq!(keys.pubkey(), s(&key["pubkey"]));
        assert_eq!(keys.npub(), s(&key["npub"]));
        assert_eq!(keys.nsec(), s(&key["nsec"]));
        // keys.ts authorIdFor, run on the same secret.
        assert_eq!(keys.author_id(), s(&key["authorId"]));
        assert_eq!(
            author_id_for(&keys.secret_hex().to_uppercase()),
            s(&key["authorId"])
        );
        // parseKey accepts the nsec, the hex (either case, padded) and returns the same key.
        assert_eq!(Keys::parse(s(&key["nsec"])).unwrap(), keys);
        assert_eq!(
            Keys::parse(&format!("  {}\n", s(&key["secret"]).to_uppercase())).unwrap(),
            keys
        );
        assert_eq!(
            npub_to_hex(s(&key["npub"])).as_deref(),
            Some(s(&key["pubkey"]))
        );
        assert_eq!(
            npub_to_hex(&s(&key["pubkey"]).to_uppercase()).as_deref(),
            Some(s(&key["pubkey"]))
        );
        assert_eq!(npub_from_hex(s(&key["pubkey"])).unwrap(), s(&key["npub"]));
    }
}

#[test]
fn nostr_tools_event_ids_and_signatures() {
    let vectors = fixture("nostr_tools_vectors.json");
    for case in vectors["events"].as_array().unwrap() {
        let keys = Keys::from_secret_hex(s(&case["secret"])).unwrap();
        let expected: Event = serde_json::from_value(case["event"].clone()).unwrap();
        let template = EventTemplate {
            kind: expected.kind,
            created_at: expected.created_at,
            tags: expected.tags.clone(),
            content: expected.content.clone(),
        };
        // Same aux randomness as @noble/curves was given: the same signature.
        let signed = keys.sign_with_aux(template.clone(), &hex32(s(&case["aux"])));
        assert_eq!(signed, expected, "{}", expected.content);
        assert!(signed.verify());
        // nostr-tools' finalizeEvent (random aux) verifies here too.
        let finalized = Event {
            sig: s(&case["finalizedSig"]).to_string(),
            ..expected.clone()
        };
        assert!(finalized.verify());
        // Our own random-aux signature is a different, valid one.
        let ours = keys.sign(template);
        assert_eq!(ours.id, expected.id);
        assert!(ours.verify());
        // Serialized like nostr-tools: the same field names, readable back.
        let json: Value = serde_json::from_str(&signed.to_json()).unwrap();
        assert_eq!(json, case["event"]);
    }
}

#[test]
fn ios_fixtures_ids_signatures_and_bech32() {
    let fx = fixture("nostr_fixtures.json");
    let keys = Keys::from_secret_hex(s(&fx["secret"])).unwrap();
    assert_eq!(keys.pubkey(), s(&fx["pubkey"]));
    assert_eq!(keys.npub(), s(&fx["npub"]));
    assert_eq!(keys.nsec(), s(&fx["nsec"]));
    assert_eq!(
        bech32::decode(s(&fx["nsec"])).unwrap(),
        ("nsec".to_string(), hex::decode(s(&fx["secret"])).unwrap())
    );
    assert!(bech32::decode("npub1garbage").is_err());
    for value in fx["events"].as_array().unwrap() {
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            event_id(
                &event.pubkey,
                event.created_at,
                event.kind,
                &event.tags,
                &event.content
            ),
            event.id
        );
        assert!(event.verify(), "{}", event.content);
    }
    for value in fx["invalid"].as_array().unwrap() {
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert!(!event.verify());
    }
}

#[test]
fn tampering_is_detected() {
    let keys = Keys::generate();
    let event = keys.sign(EventTemplate {
        kind: 1,
        created_at: 1_700_000_100,
        tags: vec![vec!["t".into(), "ünï".into()]],
        content: "sign ✓ \"quoted\" \\ back\n\ttabs 🎉".into(),
    });
    assert_eq!(event.sig.len(), 128);
    assert!(event.verify());
    let mut tampered = event.clone();
    tampered.content.push('!');
    assert!(!tampered.verify());
    // A recomputed id does not carry the old signature.
    tampered.id = event_id(
        &tampered.pubkey,
        tampered.created_at,
        tampered.kind,
        &tampered.tags,
        &tampered.content,
    );
    assert!(!tampered.verify());
    // Uppercase hex is not the canonical id.
    let upper = Event {
        id: event.id.to_uppercase(),
        ..event.clone()
    };
    assert!(!upper.verify());
    let wrong_author = Event {
        pubkey: Keys::generate().pubkey().to_string(),
        ..event
    };
    assert!(!wrong_author.verify());
}

#[test]
fn refuses_invalid_secrets() {
    assert_eq!(
        Keys::from_secret_bytes(&[1; 31]),
        Err(KeyError::InvalidSecret)
    );
    assert_eq!(
        Keys::from_secret_bytes(&[0; 32]),
        Err(KeyError::InvalidSecret)
    );
    assert_eq!(
        Keys::from_secret_bytes(&[0xff; 32]),
        Err(KeyError::InvalidSecret)
    );
    // n itself is out of range; n - 1 is the largest valid scalar.
    assert!(
        Keys::from_secret_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141")
            .is_err()
    );
    assert!(
        Keys::from_secret_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140")
            .is_ok()
    );
    for bad in [
        "",
        "nsec1",
        "npub1zsn32e6l47x6rmxy650qh8jnn7sd2t7ajmkkpklfntd3t44sttvsgg5m7h",
        "zz",
        &"g".repeat(64),
    ] {
        assert!(Keys::parse(bad).is_err(), "{bad}");
    }
    let fresh = Keys::generate();
    assert!(is_hex32(fresh.pubkey()));
    assert_ne!(Keys::generate(), fresh);
}

#[test]
fn shared_x_is_symmetric_and_rejects_bad_points() {
    let a = Keys::generate();
    let b = Keys::generate();
    assert_eq!(
        a.shared_x(b.pubkey()).unwrap(),
        b.shared_x(a.pubkey()).unwrap()
    );
    assert_eq!(a.shared_x("abc"), Err(KeyError::InvalidPubkey));
    assert_eq!(a.shared_x(&"ff".repeat(32)), Err(KeyError::InvalidPubkey));
}

/// bip-0340/test-vectors.csv: every signing row reproduces exactly, every
/// verification row gives the listed result.
#[test]
fn bip340_reference_vectors() {
    let csv = std::fs::read_to_string(format!(
        "{}/tests/fixtures/bip340_vectors.csv",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut checked = 0;
    for line in csv.lines().skip(1) {
        let cols: Vec<&str> = line.split(',').collect();
        let (index, secret, pubkey, aux, message, signature, result) = (
            cols[0],
            cols[1],
            cols[2].to_lowercase(),
            cols[3],
            cols[4],
            cols[5].to_lowercase(),
            cols[6] == "TRUE",
        );
        // Nostr signs 32-byte ids; the rows with other message lengths are not ours.
        if message.len() != 64 {
            continue;
        }
        let msg = hex32(message);
        if !secret.is_empty() {
            let keys = Keys::from_secret_hex(secret).unwrap();
            assert_eq!(keys.pubkey(), pubkey, "row {index}");
            assert_eq!(
                hex::encode(keys.sign_hash_with_aux(&msg, &hex32(aux))),
                signature,
                "row {index}"
            );
            assert!(
                verify_hash(&pubkey, &msg, &hex::encode(keys.sign_hash(&msg))),
                "row {index}"
            );
        }
        assert_eq!(
            verify_hash(&pubkey, &msg, &signature),
            result,
            "row {index}"
        );
        checked += 1;
    }
    assert!(checked >= 15);
}
