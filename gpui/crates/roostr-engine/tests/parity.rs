//! Fixture parity through the native binding: the same hand-authored and
//! reference-generated corpora glonOdin's own tests (`core/*_test.odin`), the
//! website (`scripts/parity-*.ts`) and iOS (`Tests/GlonCoreTests`) pin, so
//! this host decodes, replays, seals, authorises, serves and plans exactly
//! as they do.

#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::executor::block_on;
use roostr_engine::{
    AuthorizeSubject, Change, Engine, EngineError, MutationRequest, MutationResponse,
    ObjectHistory, OutboxPending, QueryUpdate, RelayEvent, SealRequest, SealSpace, ServingInputs,
    SessionOpen, SessionSpace, SharedProvenance, SpaceAuthority, StateInputs, VanishCandidate,
};
use serde_json::{Value, json};

fn engine() -> Engine {
    Engine::native().expect("native engine")
}

fn fixtures(name: &str) -> Value {
    let path = PathBuf::from(env!("GLON_ODIN_ROOT"))
        .join("core")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn array_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn domain(result: Result<impl std::fmt::Debug, EngineError>, context: &str) -> String {
    match result {
        Err(EngineError::Domain(message)) => message,
        other => panic!("{context}: expected a domain error, got {other:?}"),
    }
}

/// codec_fixtures.json: decode legacy bytes, re-derive the content address,
/// re-encode byte-identically, survive JSON persistence (parity-codec.ts,
/// CodecParityTests.swift).
#[test]
fn codec_fixtures() {
    let engine = engine();
    let fixtures = fixtures("codec_fixtures.json");
    let fixtures = fixtures.as_array().expect("array");
    assert!(!fixtures.is_empty());
    for fixture in fixtures {
        let name = str_of(fixture, "name");
        let id = str_of(fixture, "id");
        if let Some(malformed) = fixture["malformedBytesHex"].as_str() {
            domain(block_on(engine.decode_change(&unhex(malformed))), name);
        }
        let bytes = unhex(str_of(fixture, "bytesHex"));
        let decoded = block_on(engine.decode_change(&bytes))
            .unwrap_or_else(|e| panic!("{name}: decode: {e}"));
        assert_eq!(decoded.id, id, "{name}: embedded id");
        assert_eq!(
            block_on(engine.change_id(&decoded)).unwrap(),
            id,
            "{name}: canonical hash"
        );
        let encoded = block_on(engine.encode_change(&decoded)).unwrap();
        assert_eq!(
            hex(&encoded),
            str_of(fixture, "bytesHex"),
            "{name}: canonical encode"
        );
        assert_eq!(
            block_on(engine.decode_change(&encoded)).unwrap(),
            decoded,
            "{name}: payload round-trips"
        );
        let persisted: Change =
            serde_json::from_str(&serde_json::to_string(&decoded).unwrap()).unwrap();
        assert_eq!(
            block_on(engine.change_id(&persisted)).unwrap(),
            id,
            "{name}: JSON persistence"
        );
    }
}

/// Map insertion order is part of the legacy content address and survives the
/// binding's typed form; malformed wire and base64 are refused.
#[test]
fn codec_map_order_and_malformed_input() {
    let engine = engine();
    let pairs = json!([
        ["z", {"bytesValue": "AAH/"}],
        ["entries", {"stringValue": "opaque map key"}],
        ["__proto__", {"boolValue": false}],
        ["a", {"mapValue": {"entries": [["z", {"intValue": 1}], ["a", {"intValue": 2}]]}}],
    ]);
    let change_with = |entries: Value| -> Change {
        serde_json::from_value(json!({
            "id": "", "objectId": "maps", "parentIds": [], "timestamp": 1, "author": "a",
            "ops": [{"fieldSet": {"key": "map", "value": {"mapValue": {"entries": entries}}}}],
        }))
        .unwrap()
    };
    let change = change_with(pairs.clone());
    let mut reversed_pairs = pairs.as_array().unwrap().clone();
    reversed_pairs.reverse();
    let reversed = change_with(Value::Array(reversed_pairs));
    let roundtrip =
        block_on(engine.decode_change(&block_on(engine.encode_change(&change)).unwrap())).unwrap();
    assert_eq!(roundtrip.ops, change.ops, "ordered maps round-trip exactly");
    assert_ne!(
        block_on(engine.change_id(&change)).unwrap(),
        block_on(engine.change_id(&reversed)).unwrap(),
        "map insertion order is part of the content address"
    );

    for malformed in [
        "00",
        "0a80",
        "22021a01",
        "22041a021201",
        "3a032a0100",
        "22091a0712051900000000",
        "22020a01",
        "3a023200",
        "2880808080808080808002",
        "0affffffffffffffffff01",
    ] {
        domain(block_on(engine.decode_change(&unhex(malformed))), malformed);
    }
    for bytes in ["=", "===", "a", "AA=A", "!!!!"] {
        domain(
            block_on(engine.call("codec", json!({"action": "decode", "bytes": bytes}), None)),
            bytes,
        );
    }
    assert_eq!(
        domain(
            block_on(engine.call("nope", json!({}), None)),
            "unknown method"
        ),
        "unknown core method"
    );
}

/// replay_fixtures.json: expected states, in order, reversed and repeated
/// (parity-replay-fixtures.ts); malformed payloads are refused.
#[test]
fn replay_fixtures() {
    let engine = engine();
    let fixtures = fixtures("replay_fixtures.json");
    for fixture in fixtures.as_array().expect("array") {
        let name = str_of(fixture, "name");
        let changes: Vec<Change> =
            serde_json::from_value(fixture["changes"].clone()).expect("changes");
        let expected = (!fixture["expected"].is_null()).then(|| fixture["expected"].clone());
        assert_eq!(
            block_on(engine.replay(&changes, None)).unwrap(),
            expected,
            "{name}"
        );
        let mut reversed = changes.clone();
        reversed.reverse();
        assert_eq!(
            block_on(engine.replay(&reversed, None)).unwrap(),
            expected,
            "{name}: reversed"
        );
        assert_eq!(
            block_on(engine.replay(&changes, None)).unwrap(),
            expected,
            "{name}: repeated"
        );

        // Without a base, replay_from_base is the plain replay plus its sorted heads.
        let inputs = StateInputs {
            changes: changes.clone(),
            ..StateInputs::default()
        };
        let from_base = block_on(engine.replay_from_base(&inputs)).unwrap();
        let mut heads = block_on(engine.heads(&changes, None)).unwrap();
        heads.sort();
        let expected_with_heads = expected.clone().map(|mut state| {
            state["heads"] = json!(heads);
            state
        });
        assert_eq!(
            from_base, expected_with_heads,
            "{name}: replay_from_base without a base"
        );
        if !changes.is_empty() {
            let classification = block_on(engine.classify(&inputs)).unwrap();
            assert!(classification.base.is_none(), "{name}: no base");
            assert_eq!(
                classification.live.len(),
                changes.len(),
                "{name}: every change live"
            );
            assert_eq!(
                block_on(engine.object_heads(&inputs)).unwrap().len(),
                heads.len(),
                "{name}: heads"
            );
        }
    }
    for payload in [
        Value::Null,
        json!({}),
        json!({"changes": {}}),
        json!({"changes": [null]}),
    ] {
        domain(
            block_on(engine.call("replay", payload.clone(), None)),
            &payload.to_string(),
        );
    }
}

/// wire_fixtures.json, produced by the TypeScript reference (nostr-tools
/// NIP-44 plus the old sync.ts wire code): every byte must match.
#[test]
fn wire_fixtures() {
    let engine = engine();
    let fixtures = fixtures("wire_fixtures.json");
    for fixture in array_of(&fixtures, "blind") {
        let id = str_of(fixture, "id");
        let blinded = match fixture["secret"].as_str() {
            Some(secret) => block_on(engine.blind_personal(secret, id)),
            None => block_on(engine.blind_space(str_of(fixture, "keyHex"), id)),
        };
        assert_eq!(
            blinded.unwrap(),
            str_of(fixture, "expected"),
            "blind {}",
            str_of(fixture, "name")
        );
    }
    for fixture in array_of(&fixtures, "conversationKeys") {
        assert_eq!(
            block_on(engine.conversation_key(str_of(fixture, "sharedX"))).unwrap(),
            str_of(fixture, "expected"),
            "conversation key"
        );
    }
    for fixture in array_of(&fixtures, "seal") {
        let name = str_of(fixture, "name");
        let change_text = str_of(fixture, "change");
        let change = STANDARD.decode(change_text).unwrap();
        let space = fixture.get("space").map(|space| SealSpace {
            key_hex: str_of(space, "keyHex"),
            space_id: str_of(space, "spaceId"),
            owner: space["owner"].as_str(),
        });
        let conversation_key = str_of(fixture, "conversationKey");
        let sealed = block_on(engine.seal(&SealRequest {
            change: &change,
            conversation_key,
            secret: fixture["secret"].as_str(),
            space,
            object_id: str_of(fixture, "objectId"),
            base: fixture["base"].as_str(),
            nonce: fixture["nonce"].as_str(),
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&sealed).unwrap(),
            fixture["expected"],
            "seal {name}"
        );
        for (index, part) in sealed.parts.iter().enumerate() {
            let chunk = &change_text[(index * 40_000).min(change_text.len())
                ..((index + 1) * 40_000).min(change_text.len())];
            assert_eq!(
                block_on(engine.open(&part.content, conversation_key)).unwrap(),
                chunk,
                "seal {name}: part {index} opens to its chunk"
            );
        }
    }
    for fixture in array_of(&fixtures, "open") {
        let name = str_of(fixture, "name");
        let opened = block_on(engine.open(
            str_of(fixture, "content"),
            str_of(fixture, "conversationKey"),
        ));
        if fixture["error"].as_bool() == Some(true) {
            domain(opened, name);
        } else {
            assert_eq!(opened.unwrap(), str_of(fixture, "expected"), "open {name}");
        }
    }
    for fixture in array_of(&fixtures, "verify") {
        let name = str_of(fixture, "name");
        let verified = block_on(engine.verify(str_of(fixture, "bytes"), fixture["gid"].as_str()));
        if fixture["error"].as_bool() == Some(true) {
            domain(verified, name);
            continue;
        }
        let verified = verified.unwrap();
        assert_eq!(
            verified.id,
            str_of(fixture, "expectedId"),
            "verify {name}: raw content address"
        );
        assert_eq!(
            verified.change.id,
            str_of(fixture, "expectedId"),
            "verify {name}: decoded id"
        );
    }
}

/// serving_fixtures.json (parity-serving.ts, ServingContractTests.swift).
#[test]
fn serving_fixtures() {
    let engine = engine();
    let fixtures = fixtures("serving_fixtures.json");
    for fixture in array_of(&fixtures, "resolve") {
        let name = str_of(fixture, "name");
        let inputs = ServingInputs {
            object: fixture.get("object").filter(|object| !object.is_null()),
            agents: array_of(fixture, "agents"),
            machines: array_of(fixture, "machines"),
            skills: array_of(fixture, "skills"),
            capabilities: array_of(fixture, "capabilities"),
        };
        let resolved = block_on(engine.resolve_serving(&inputs));
        if let Some(error) = fixture["error"].as_str() {
            assert_eq!(domain(resolved, name), error, "resolve {name}");
        } else {
            let resolved = resolved.unwrap_or_else(|e| panic!("{name}: {e}"));
            let expected = &fixture["expected"];
            assert_eq!(
                resolved.machine_id,
                str_of(expected, "machineId"),
                "resolve {name}: machine"
            );
            assert_eq!(
                resolved.reason,
                str_of(expected, "reason"),
                "resolve {name}: reason"
            );
            assert_eq!(
                json!(resolved.skills),
                expected["skills"],
                "resolve {name}: skills"
            );
            assert_eq!(
                json!(resolved.candidates),
                expected["candidates"],
                "resolve {name}: candidates"
            );
        }
    }
}

/// authority_fixtures.json (parity-authority.ts, AuthorityContractTests.swift).
#[test]
fn authority_fixtures() {
    let engine = engine();
    let fixtures = fixtures("authority_fixtures.json");
    for fixture in array_of(&fixtures, "authorize") {
        let name = str_of(fixture, "name");
        let change: Change = serde_json::from_value(fixture["change"].clone()).expect("change");
        let provenance: SharedProvenance =
            serde_json::from_value(fixture["provenance"].clone()).expect("provenance");
        let space: SpaceAuthority =
            serde_json::from_value(fixture["space"].clone()).expect("space");
        let verdict = block_on(engine.authorize(
            AuthorizeSubject::Change(&change),
            &provenance,
            &space,
            fixture.get("trustedSpace").filter(|v| !v.is_null()),
            fixture.get("existing").filter(|v| !v.is_null()),
        ));
        if let Some(error) = fixture["error"].as_str() {
            assert_eq!(domain(verdict, name), error, "authorize {name}");
            continue;
        }
        let verdict = verdict.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            Some(verdict.ok),
            fixture["ok"].as_bool(),
            "authorize {name}: ok"
        );
        assert_eq!(
            verdict.reason,
            str_of(fixture, "reason"),
            "authorize {name}: reason"
        );
    }
    for fixture in array_of(&fixtures, "vanished") {
        let name = str_of(fixture, "name");
        let objects: Vec<VanishCandidate> = array_of(fixture, "objects")
            .iter()
            .map(|object| VanishCandidate {
                id: str_of(object, "id").into(),
                channel: str_of(object, "channel").into(),
            })
            .collect();
        let actual =
            block_on(engine.vanished(fixture.get("ledger").filter(|v| !v.is_null()), &objects))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        let actual: BTreeMap<String, (i64, bool)> = actual
            .into_iter()
            .map(|entry| (entry.object_id, (entry.at, entry.left)))
            .collect();
        let expected: BTreeMap<String, (i64, bool)> = array_of(fixture, "expected")
            .iter()
            .map(|entry| {
                let left = entry["left"].as_bool().unwrap_or(false);
                (
                    str_of(entry, "objectId").to_string(),
                    (entry["at"].as_i64().unwrap(), left),
                )
            })
            .collect();
        assert_eq!(
            actual.len(),
            array_of(fixture, "expected").len(),
            "vanished {name}: entry count"
        );
        assert_eq!(actual, expected, "vanished {name}");
    }
}

/// mutation_fixtures.json: plans compared as canonical protobuf, as
/// parity-mutation-fixtures.ts and mutation_test.odin do.
#[test]
fn mutation_fixtures() {
    let engine = engine();
    let fixtures = fixtures("mutation_fixtures.json");
    let fixtures = fixtures.as_array().expect("array");
    assert!(!fixtures.is_empty());
    for fixture in fixtures {
        let name = str_of(fixture, "name");
        let payload = &fixture["payload"];
        let request = MutationRequest {
            action: str_of(payload, "action"),
            params: &payload["params"],
            timestamp: payload["timestamp"].as_i64().unwrap_or_default(),
            author: str_of(payload, "author"),
            id_seed: str_of(payload, "id_seed"),
            key_id: payload["key_id"].as_i64().unwrap_or_default(),
            objects: payload["objects"].as_array().map(Vec::as_slice),
            overlay: &[],
            absent: &[],
        };
        let response = block_on(engine.mutation(&request));
        if fixture["error"].as_bool() == Some(true) {
            let message = domain(response, name);
            if let Some(expected) = fixture["expected_error"].as_str() {
                assert_eq!(message, expected, "{name}: error");
            }
            continue;
        }
        let MutationResponse::Plan(plan) = response.unwrap_or_else(|e| panic!("{name}: {e}"))
        else {
            panic!("{name}: planned over explicit objects, nothing can be missing");
        };
        assert_eq!(plan.result, fixture["expected_result"], "{name}: result");
        assert_eq!(
            json!(plan.vanish_ids),
            fixture["expected_vanish_ids"],
            "{name}: vanish ids"
        );
        for (key, actual) in [
            ("changes", &plan.changes),
            ("vanish_changes", &plan.vanish_changes),
        ] {
            let expected: Vec<Change> =
                serde_json::from_value(fixture[format!("expected_{key}")].clone())
                    .expect("expected changes");
            assert_eq!(actual.len(), expected.len(), "{name}: {key} count");
            for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                // Encoding compares the full operation model including ordered maps.
                let actual_bytes = block_on(engine.encode_change(actual)).unwrap();
                let expected_bytes = block_on(engine.encode_change(expected)).unwrap();
                assert_eq!(actual_bytes, expected_bytes, "{name}: {key}[{index}]");
                block_on(engine.decode_change(&actual_bytes))
                    .unwrap_or_else(|e| panic!("{name}: decodes: {e}"));
            }
        }
    }
}

/// The receive session and outbox, driven like SyncSessionTests.swift: the
/// engine's own seal produces the events, the session reassembles and
/// verifies them, and the outbox seals on first attempt. Also the corpus
/// load's framing and the cache it fills. One test: both are engine-global.
#[test]
fn session_outbox_and_corpus() {
    let engine = engine();
    let pk = "ab".repeat(32);
    let secret = "01".repeat(32);
    let conversation_key = "02".repeat(32);
    let space_key = "03".repeat(32);
    let state = block_on(engine.session_open(&SessionOpen {
        pk: pk.clone(),
        conversation_key: conversation_key.clone(),
        secret: Some(secret.clone()),
        spaces: vec![SessionSpace {
            space_id: "space-1".into(),
            key_hex: space_key,
            key_id: 1,
            owner: None,
        }],
        cursor: 10,
        ..SessionOpen::default()
    }))
    .unwrap();
    assert_eq!(
        (state.cursor, state.groups, state.replay_groups.len()),
        (10, 0, 0)
    );

    let change: Change = serde_json::from_value(json!({
        "objectId": "note-1", "parentIds": [], "timestamp": 1, "author": "t",
        "ops": [{"objectCreate": {"typeKey": "page"}}],
    }))
    .unwrap();
    let bytes = block_on(engine.encode_change(&change)).unwrap();
    let id = block_on(engine.change_id(&change)).unwrap();
    let sealed = block_on(engine.seal(&SealRequest {
        change: &bytes,
        conversation_key: &conversation_key,
        secret: Some(&secret),
        space: None,
        object_id: "note-1",
        base: None,
        nonce: None,
    }))
    .unwrap();
    assert_eq!(sealed.parts.len(), 1);
    let event = RelayEvent {
        pubkey: pk.clone(),
        created_at: 100,
        kind: roostr_engine::CHANGE_KIND,
        tags: sealed.parts[0].tags.clone(),
        content: sealed.parts[0].content.clone(),
    };
    let ingested = block_on(engine.ingest(&event, 1_000)).unwrap();
    assert_eq!(ingested.cursor, 100, "cursor advances to the event");
    let item = ingested.item.expect("a single part yields an item");
    assert_eq!(item.bytes, bytes, "the item carries the wire bytes");
    assert_eq!(item.change.expect("a change").id, id);
    assert!(
        item.provenance.is_none(),
        "personal events carry no provenance"
    );
    let foreign = RelayEvent {
        pubkey: "ef".repeat(32),
        ..event
    };
    let rejected = block_on(engine.ingest(&foreign, 1_000)).unwrap();
    assert!(
        rejected.item.is_none(),
        "a personal event from another pubkey is not ours"
    );

    let pending = OutboxPending {
        key: format!("personal:{id}"),
        object_id: "note-1".into(),
        change_id: id.clone(),
        bytes: bytes.clone(),
        space_id: None,
        key_id: None,
        base: None,
        has_events: false,
    };
    let queued = block_on(engine.outbox_enqueue(&pending)).unwrap();
    assert!(queued.queued);
    assert_eq!(queued.pending, 1);
    let again = block_on(engine.outbox_enqueue(&pending)).unwrap();
    assert!(!again.queued, "dedupe by key");
    let next = block_on(engine.outbox_next(2_000)).unwrap();
    let item = next.item.expect("the queued item is due");
    assert_eq!(item.change_id, id);
    let sealed = item.sealed.expect("sealed on first attempt");
    let opened = block_on(engine.open(&sealed.parts[0].content, &conversation_key)).unwrap();
    assert_eq!(
        opened,
        STANDARD.encode(&bytes),
        "the outbox seals the change bytes"
    );
    assert_eq!(
        block_on(engine.outbox_result(&item.key, true, true, 2_100)).unwrap(),
        0
    );
    assert!(block_on(engine.session_close()).unwrap());
    domain(block_on(engine.session_state()), "state without a session");

    // Corpus framing: each replay fixture's history, as stored protobuf, loads
    // as one object. (Re-encoding re-addresses the fixtures' synthetic ids, so
    // their parent links - and states - are not the fixtures' any more.)
    let body = json!({"limit": 1000});
    let list = |removed: &[String]| {
        block_on(engine.query(QueryUpdate {
            upserts: &[],
            removed,
            reset: false,
            body: Some(&body),
            now_ms: 0,
        }))
        .unwrap()
    };
    let fixtures = fixtures("replay_fixtures.json");
    for fixture in fixtures.as_array().expect("array") {
        let name = str_of(fixture, "name");
        let changes: Vec<Change> =
            serde_json::from_value(fixture["changes"].clone()).expect("changes");
        if changes.is_empty() {
            continue;
        }
        let history = ObjectHistory {
            checkpoint: None,
            changes: changes
                .iter()
                .map(|change| block_on(engine.encode_change(change)).unwrap())
                .collect(),
        };
        let load = block_on(engine.load_corpus([&history], true, 0)).unwrap();
        let framed: usize = history.changes.iter().map(|bytes| 4 + bytes.len()).sum();
        assert_eq!(
            (
                load.objects,
                load.changes,
                load.skipped,
                load.cached,
                load.bytes as usize
            ),
            (1, changes.len() as u64, 0, 1, framed),
            "corpus {name}"
        );
    }

    // Several objects in one load, then a committed change applied to the cache.
    let mut histories = Vec::new();
    let mut heads = Vec::new();
    for index in 0..5 {
        let change: Change = serde_json::from_value(json!({
            "objectId": format!("obj-{index}"), "parentIds": [], "timestamp": 1, "author": "t",
            "ops": [{"objectCreate": {"typeKey": "page"}}, {"fieldSet": {"key": "name", "value": {"stringValue": format!("page {index}")}}}],
        }))
        .unwrap();
        heads.push(block_on(engine.change_id(&change)).unwrap());
        histories.push(ObjectHistory {
            checkpoint: None,
            changes: vec![block_on(engine.encode_change(&change)).unwrap()],
        });
    }
    let load = block_on(engine.load_corpus(&histories, true, 0)).unwrap();
    assert_eq!((load.objects, load.changes, load.cached), (5, 5, 5));
    assert_eq!(list(&[]).total, 5);
    let mut rename: Change = serde_json::from_value(json!({
        "objectId": "obj-0", "parentIds": [heads[0]], "timestamp": 2, "author": "t",
        "ops": [{"fieldSet": {"key": "name", "value": {"stringValue": "renamed"}}}],
    }))
    .unwrap();
    rename.id = block_on(engine.change_id(&rename)).unwrap();
    let applied = block_on(engine.apply_committed(std::slice::from_ref(&rename))).unwrap();
    assert_eq!(
        applied.applied,
        vec![rename.id.clone()],
        "applied on exactly the cached heads"
    );
    let refused = block_on(engine.apply_committed(&[rename])).unwrap();
    assert_eq!(
        refused.applied.len(),
        0,
        "the cache moved on: a stale parent is refused"
    );
    let listed = list(&["obj-1".to_string()]);
    assert_eq!(listed.total, 4, "removed ids leave the cache");
    let renamed = listed
        .records
        .iter()
        .find(|row| row["id"] == "obj-0")
        .expect("obj-0");
    assert_eq!(renamed["name"], "renamed");

    let empty = block_on(engine.load_corpus(std::iter::empty(), true, 0)).unwrap();
    assert_eq!(empty, roostr_engine::CorpusLoad::default());
    assert_eq!(list(&[]).total, 0, "an empty reset load clears the cache");
}

/// The fingerprint follows build-xcframework.mjs, so it equals the iOS
/// vendored manifest whenever both were built from the same sources.
#[test]
fn source_fingerprint_is_a_sha256() {
    let fingerprint = engine().source_fingerprint().to_string();
    assert_eq!(fingerprint.len(), 64);
    assert!(fingerprint.bytes().all(|b| b.is_ascii_hexdigit()));
}
