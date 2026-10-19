//! Negentropy V1 against transcripts of the reference implementation
//! (hoytech/negentropy js/Negentropy.js): the iOS suite's
//! `negentropy_vectors.json` and the golden values of the Svelte site's
//! `scripts/negentropy.test.ts`. Every message either side produces must
//! match byte for byte.

use roostr_nostr::negentropy::{Item, Negentropy, NegentropyError, NegentropyStorage, varint};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn storage(items: &[(u64, String)]) -> NegentropyStorage {
    NegentropyStorage::from_hex(items.iter().map(|(t, id)| (*t, id.as_str()))).unwrap()
}

struct Run {
    messages: Vec<Vec<u8>>,
    have: Vec<String>,
    need: Vec<String>,
}

/// Drive an initiator against a responder to completion.
fn run(mut client: Negentropy, mut server: Negentropy) -> Run {
    let mut out = Run {
        messages: Vec::new(),
        have: Vec::new(),
        need: Vec::new(),
    };
    let mut query = client.initiate().unwrap();
    loop {
        out.messages.push(query.clone());
        let answer = server.reconcile(&query).unwrap();
        assert!(
            answer.have_ids.is_empty() && answer.need_ids.is_empty(),
            "the server never collects ids"
        );
        let reply = answer.output.expect("the server always answers");
        out.messages.push(reply.clone());
        let step = client.reconcile(&reply).unwrap();
        out.have.extend(step.have_ids);
        out.need.extend(step.need_ids);
        match step.output {
            Some(next) => query = next,
            None => return out,
        }
        assert!(out.messages.len() < 1000);
    }
}

#[test]
fn reference_transcripts() {
    let path = format!(
        "{}/tests/fixtures/negentropy_vectors.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let scenarios: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(!scenarios.is_empty());
    for s in &scenarios {
        let name = s["name"].as_str().unwrap();
        let num = |k: &str| s[k].as_u64().unwrap();
        // The generator's sets: id = sha256("seed:i"), LCG timestamps and membership.
        let seed = num("seed") as u32;
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x
        };
        let (mut client, mut server) = (Vec::new(), Vec::new());
        for i in 0..num("n") {
            let id = sha(format!("{seed}:{i}").as_bytes());
            let ts = 1_677_970_534 + u64::from(next() % num("spread") as u32);
            let r = u64::from(next() % 1000);
            if r < num("onlyClient") {
                client.push((ts, id));
            } else if r < num("onlyClient") + num("onlyServer") {
                server.push((ts, id));
            } else {
                client.push((ts, id.clone()));
                server.push((ts, id));
            }
        }
        assert_eq!(client.len() as u64, num("clientCount"), "{name}");
        assert_eq!(server.len() as u64, num("serverCount"), "{name}");
        let result = run(
            Negentropy::new(storage(&client), num("clientLimit") as usize).unwrap(),
            Negentropy::new(storage(&server), num("serverLimit") as usize).unwrap(),
        );
        if let Some(expected) = s["messages"].as_array() {
            let ours: Vec<String> = result.messages.iter().map(hex::encode).collect();
            let theirs: Vec<&str> = expected.iter().map(|m| m.as_str().unwrap()).collect();
            assert_eq!(ours, theirs, "{name}");
        }
        let hashes: Vec<String> = result.messages.iter().map(|m| sha(m)).collect();
        let expected: Vec<&str> = s["messageHashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h.as_str().unwrap())
            .collect();
        assert_eq!(hashes, expected, "{name}");
        let (mut have, mut need) = (result.have.clone(), result.need.clone());
        assert_eq!(have.len() as u64, num("haveCount"), "{name}");
        assert_eq!(need.len() as u64, num("needCount"), "{name}");
        have.sort();
        need.sort();
        assert_eq!(
            sha(have.join(",").as_bytes()),
            s["haveHash"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            sha(need.join(",").as_bytes()),
            s["needHash"].as_str().unwrap(),
            "{name}"
        );
        // The sets differ by exactly have ∪ need.
        let ids = |set: &[(u64, String)]| {
            set.iter()
                .map(|(_, id)| id.clone())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let (a, b) = (ids(&client), ids(&server));
        assert_eq!(
            need.iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            &b - &a,
            "{name}"
        );
        assert_eq!(
            have.iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            &a - &b,
            "{name}"
        );
    }
}

// scripts/negentropy.test.ts: clustered timestamps, eight items per second,
// so bounds need id prefixes.
fn id_of(i: u64) -> String {
    sha(format!("roostr-neg-{i}").as_bytes())
}

fn ts(i: u64) -> u64 {
    1_700_000_000 + (i >> 3)
}

fn ts_storage(items: impl IntoIterator<Item = u64>) -> NegentropyStorage {
    storage(
        &items
            .into_iter()
            .map(|i| (ts(i), id_of(i)))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn ts_fingerprints_and_initial_messages() {
    let mut empty = Negentropy::new(ts_storage([]), 0).unwrap();
    assert_eq!(hex::encode(empty.initiate().unwrap()), "6100000200");
    assert_eq!(
        hex::encode(ts_storage([]).fingerprint(0, 0)),
        "7f9c9e31ac8256ca2f258583df262dbc"
    );
    let small = storage(
        &[0u64, 1, 2]
            .iter()
            .map(|&i| (ts(i * 9), id_of(i)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        hex::encode(small.fingerprint(0, 3)),
        "3dfcd3fed9b374242a7469a5721fbf07"
    );
    assert_eq!(
        hex::encode(Negentropy::new(small, 0).unwrap().initiate().unwrap()),
        "610000020386731aa29bf41d347d430bcdcedb105385cfedb662d460726784000877cf97de415adbd8b560618864dcd842bf40739b9923e235c561492ed97c281b0c9e9db6513fdf2cc148bea5b526b937c5d568e347ed810e78e0a4ae6601fa9e65c87d9a"
    );
}

#[test]
fn ts_framed_multi_round_reconciliation() {
    let client = (0..2000).filter(|i| i % 7 != 0);
    let server = (0..2100).filter(|i| i % 11 != 0 || *i >= 2000);
    let r = run(
        Negentropy::new(ts_storage(client), 4096).unwrap(),
        Negentropy::new(ts_storage(server), 4096).unwrap(),
    );
    let trace: Vec<String> = r.messages.iter().map(hex::encode).collect();
    assert_eq!(trace.len() / 2, 22);
    assert_eq!(&trace[0][..40], "6186aacfe21001cb012672eecd18cf0ee328b906");
    assert_eq!(
        sha(trace.join("\n").as_bytes()),
        "d38c183f8340dc069acc7da539d86c8eecdc00e65a0a2650a1d06830199dc0d5"
    );
    assert_eq!(r.need.len(), 360);
    assert_eq!(
        sha(r.need.join(",").as_bytes()),
        "eb4fefb07bbe99808bdef3b95c64068eb90625566c67bfbec61e962d4ff594bb"
    );
    assert_eq!(r.have.len(), 156);
    assert_eq!(
        sha(r.have.join(",").as_bytes()),
        "4c0b7417418b516005f035eaf8ef6c3b91455fdb354dfa927aadff139c5c31b3"
    );
    assert!(r.messages.iter().all(|m| m.len() <= 4096));
}

#[test]
fn ts_set_differences() {
    let server: Vec<u64> = (0..5000).collect();
    let r = run(
        Negentropy::new(ts_storage([]), 250_000).unwrap(),
        Negentropy::new(ts_storage(server.clone()), 250_000).unwrap(),
    );
    assert!(r.have.is_empty());
    let need: std::collections::BTreeSet<_> = r.need.into_iter().collect();
    assert_eq!(need, server.iter().map(|&i| id_of(i)).collect());
    let same = run(
        Negentropy::new(ts_storage(server.clone()), 250_000).unwrap(),
        Negentropy::new(ts_storage(server), 250_000).unwrap(),
    );
    assert_eq!(same.messages.len(), 2);
    assert!(same.need.is_empty());
}

#[test]
fn varints_and_malformed_input() {
    let encoded: Vec<u8> = [0u64, 1, 127, 128, 16383, 16384, 1_677_970_534]
        .into_iter()
        .flat_map(varint)
        .collect();
    assert_eq!(hex::encode(encoded), "00017f8100ff7f81800086a08f9866");

    assert_eq!(
        NegentropyStorage::from_hex([(1, "zz")]).unwrap_err(),
        NegentropyError::BadId
    );
    let id = "ab".repeat(32);
    assert_eq!(
        NegentropyStorage::from_hex([(1, id.as_str()), (1, id.as_str())]).unwrap_err(),
        NegentropyError::DuplicateItem
    );
    assert_eq!(
        NegentropyStorage::new([Item {
            timestamp: u64::MAX,
            id: [0; 32]
        }])
        .unwrap_err(),
        NegentropyError::BadTimestamp
    );
    assert!(matches!(
        Negentropy::new(ts_storage([]), 100),
        Err(NegentropyError::FrameSizeLimitTooSmall)
    ));

    let mut client = Negentropy::new(ts_storage([1]), 0).unwrap();
    client.initiate().unwrap();
    assert_eq!(
        client.initiate().unwrap_err(),
        NegentropyError::AlreadyInitiated
    );
    assert_eq!(
        client.reconcile(&[0x62]).unwrap_err(),
        NegentropyError::UnsupportedVersion(0x62)
    );
    assert_eq!(
        client.reconcile(&[0x10]).unwrap_err(),
        NegentropyError::InvalidVersion(0x10)
    );
    assert_eq!(
        client.reconcile(&[0x61, 0x01, 0x01, 0x01]).unwrap_err(),
        NegentropyError::ParseEndsPrematurely
    );
    // Five ids promised, none sent.
    assert_eq!(
        client
            .reconcile(&[0x61, 0x00, 0x00, 0x02, 0x05])
            .unwrap_err(),
        NegentropyError::ParseEndsPrematurely
    );
    assert_eq!(
        client.reconcile(&[0x61, 0x00, 0x21]).unwrap_err(),
        NegentropyError::BoundKeyTooLong
    );
    // A responder answers another version with a bare version byte.
    let mut server = Negentropy::new(ts_storage([1]), 0).unwrap();
    assert_eq!(server.reconcile(&[0x62]).unwrap().output, Some(vec![0x61]));
}
