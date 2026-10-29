//! State records (glonOdin `docs/state-objects.md`): merge laws, clocks,
//! the view, and byte parity with the Odin engine through the shared vectors
//! in `glonOdin/core/testdata/record_vectors/`.

#![cfg(not(target_family = "wasm"))]

use roostr_engine::record::{
    Body, Content, Entry, EntryBody, Hlc, Patch, Record, Stamp, Value, hlc_now, hlc_observe,
    key_between, record_apply_patch, record_canonical, record_merge, record_view,
    sibling_positions,
};
use serde_json::json;

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i as u64 + 1) as usize);
        }
    }
}

fn stamp(ms: u64, device: u32) -> Stamp {
    Stamp {
        time: ms << 16,
        device,
    }
}

fn field(key: &str, at: Stamp, value: Value) -> Entry {
    Entry {
        key: key.into(),
        stamp: at,
        gone: false,
        body: EntryBody::Value(value),
    }
}

fn text(text: &str) -> Content {
    Content::Text {
        text: text.into(),
        style: 0,
        marks: Vec::new(),
        checked: false,
        color: String::new(),
    }
}

fn block(key: &str, at: Stamp, parent: &str, position: &str, content: &str) -> Entry {
    Entry {
        key: key.into(),
        stamp: at,
        gone: false,
        body: EntryBody::Block(Body {
            parent: parent.into(),
            position: position.into(),
            content: text(content),
            ..Body::default()
        }),
    }
}

fn tombstone(key: &str, at: Stamp, body: EntryBody) -> Entry {
    Entry {
        key: key.into(),
        stamp: at,
        gone: true,
        body,
    }
}

/// A random record over a small key space, so copies collide often
/// (including equal stamps with different payloads).
fn random_record(rng: &mut Rng) -> Record {
    let mut record = Record {
        object_id: "obj".into(),
        created_at: rng.below(3) as i64 * 10,
        ..Record::default()
    };
    let at = |rng: &mut Rng| stamp(rng.below(4), rng.below(3) as u32);
    if rng.below(2) == 0 {
        record.kind = Some(field("type", at(rng), Value::String(format!("t{}", rng.below(3)))));
    }
    if rng.below(2) == 0 {
        record.deleted = Some(field("deleted", at(rng), Value::Bool(rng.below(2) == 0)));
    }
    for _ in 0..rng.below(6) {
        let key = format!("f{}", rng.below(4));
        let entry = match rng.below(4) {
            0 => tombstone(&key, at(rng), EntryBody::Value(Value::None)),
            1 => field(
                &key,
                at(rng),
                Value::Map(vec![("b".into(), Value::Int(1)), ("a".into(), Value::Float(0.5))]),
            ),
            _ => field(&key, at(rng), Value::Int(rng.below(5) as i64)),
        };
        record.fields.push(entry);
    }
    for _ in 0..rng.below(6) {
        let key = format!("b{}", rng.below(4));
        let entry = if rng.below(4) == 0 {
            tombstone(&key, at(rng), EntryBody::Block(Body::default()))
        } else {
            block(&key, at(rng), "", &format!("{}", rng.below(3)), &format!("x{}", rng.below(3)))
        };
        record.blocks.push(entry);
    }
    for _ in 0..rng.below(3) {
        let key = format!("m{}", rng.below(3));
        let mut entry = block(&key, at(rng), "b0", "V", "hi");
        if let EntryBody::Block(body) = entry.body {
            entry.body = EntryBody::Message(body);
        }
        record.messages.push(entry);
    }
    record
}

fn merge_all(records: &[Record]) -> Record {
    records
        .iter()
        .fold(Record::default(), |acc, record| record_merge(&acc, record).expect("merge"))
}

#[test]
fn merge_is_commutative_associative_idempotent() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for _ in 0..2000 {
        let copies: Vec<Record> = (0..1 + rng.below(5)).map(|_| random_record(&mut rng)).collect();
        let expected = record_canonical(&merge_all(&copies));
        // Shuffled and duplicated.
        let mut shuffled: Vec<Record> = copies
            .iter()
            .flat_map(|record| std::iter::repeat_n(record.clone(), 1 + rng.below(3) as usize))
            .collect();
        rng.shuffle(&mut shuffled);
        assert_eq!(record_canonical(&merge_all(&shuffled)), expected);
        // Pairwise laws.
        let (a, b) = (&copies[0], &copies[copies.len() - 1]);
        let c = random_record(&mut rng);
        let ab = record_merge(a, b).unwrap();
        assert_eq!(record_canonical(&ab), record_canonical(&record_merge(b, a).unwrap()));
        assert_eq!(record_canonical(&record_merge(&ab, &ab).unwrap()), record_canonical(&ab));
        let left = record_merge(&ab, &c).unwrap();
        let right = record_merge(a, &record_merge(b, &c).unwrap()).unwrap();
        assert_eq!(record_canonical(&left), record_canonical(&right));
    }
}

#[test]
fn patches_converge_in_any_order() {
    let mut rng = Rng(0x0dd5_eed5_0000_0042);
    for _ in 0..1000 {
        let patches: Vec<Patch> = (0..1 + rng.below(6))
            .map(|_| {
                let record = random_record(&mut rng);
                let mut entries: Vec<Entry> = record.kind.into_iter().chain(record.deleted).collect();
                entries.extend(record.fields);
                entries.extend(record.blocks);
                entries.extend(record.messages);
                Patch {
                    object_id: "obj".into(),
                    entries,
                }
            })
            .collect();
        let apply = |order: &[Patch]| {
            order.iter().fold(Record::default(), |acc, patch| {
                record_apply_patch(&acc, patch).expect("patch")
            })
        };
        let expected = record_canonical(&apply(&patches));
        let mut order: Vec<Patch> = patches.iter().chain(&patches).cloned().collect();
        rng.shuffle(&mut order);
        assert_eq!(record_canonical(&apply(&order)), expected);
    }
}

#[test]
fn patch_routing_and_errors() {
    let patch = Patch {
        object_id: "obj".into(),
        entries: vec![
            field("type", stamp(1, 0), Value::String("page".into())),
            field("deleted", stamp(1, 0), Value::Bool(false)),
            field("name", stamp(1, 0), Value::String("n".into())),
            block("b", stamp(1, 0), "", "V", "x"),
        ],
    };
    let record = record_apply_patch(&Record::default(), &patch).unwrap();
    assert!(record.kind.is_some() && record.deleted.is_some());
    assert_eq!((record.fields.len(), record.blocks.len()), (1, 1));
    let bad = Patch {
        object_id: "obj".into(),
        entries: vec![Entry {
            key: "x".into(),
            ..Entry::default()
        }],
    };
    assert!(record_apply_patch(&record, &bad).is_err());
    let other = Record {
        object_id: "other".into(),
        ..Record::default()
    };
    assert!(record_merge(&record, &other).is_err());
}

#[test]
fn tombstones_order_against_edits() {
    let edit = Record {
        object_id: "obj".into(),
        blocks: vec![block("b", stamp(10, 0), "", "V", "edited")],
        ..Record::default()
    };
    let removed_later = Record {
        object_id: "obj".into(),
        blocks: vec![tombstone("b", stamp(11, 1), EntryBody::Block(Body::default()))],
        ..Record::default()
    };
    let removed_earlier = Record {
        object_id: "obj".into(),
        blocks: vec![tombstone("b", stamp(9, 1), EntryBody::Block(Body::default()))],
        ..Record::default()
    };
    for (a, b) in [(&edit, &removed_later), (&removed_later, &edit)] {
        let view = record_view(&record_merge(a, b).unwrap());
        assert_eq!(view["blocks"], json!([]));
    }
    for (a, b) in [(&edit, &removed_earlier), (&removed_earlier, &edit)] {
        let view = record_view(&record_merge(a, b).unwrap());
        assert_eq!(view["blocks"][0]["content"]["text"]["text"], "edited");
    }
    // A late-arriving older copy cannot resurrect a removed field.
    let old = Record {
        object_id: "obj".into(),
        fields: vec![field("name", stamp(5, 0), Value::String("old".into()))],
        ..Record::default()
    };
    let gone = Record {
        object_id: "obj".into(),
        fields: vec![tombstone("name", stamp(6, 0), EntryBody::Value(Value::None))],
        ..Record::default()
    };
    let merged = record_merge(&gone, &old).unwrap();
    assert_eq!(record_view(&merged)["fields"], json!({}));
    assert_eq!(record_view(&merged)["updatedAt"], 6);
}

#[test]
fn concurrent_inserts_at_one_spot_are_both_kept() {
    let first = key_between("", None);
    let second = key_between(&first, None);
    let between = key_between(&first, Some(&second));
    let base = Record {
        object_id: "obj".into(),
        blocks: vec![
            block("a", stamp(1, 0), "", &first, "a"),
            block("c", stamp(1, 0), "", &second, "c"),
        ],
        ..Record::default()
    };
    let mut left = base.clone();
    left.blocks.push(block("x", stamp(5, 1), "", &between, "from 1"));
    let mut right = base.clone();
    right.blocks.push(block("y", stamp(5, 2), "", &between, "from 2"));
    let lr = record_view(&record_merge(&left, &right).unwrap());
    let rl = record_view(&record_merge(&right, &left).unwrap());
    assert_eq!(lr, rl);
    let ids: Vec<&str> = lr["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "x", "y", "c"]);
}

#[test]
fn view_builds_the_tree() {
    let mut message = block("m", stamp(3, 0), "root", "V", "");
    message.body = EntryBody::Message(Body {
        parent: "root".into(),
        position: "V".into(),
        content: Content::Custom {
            content_type: "chat".into(),
            data: vec![1, 2],
            meta: vec![("text".into(), "hi".into())],
        },
        ..Body::default()
    });
    let record = Record {
        object_id: "obj".into(),
        kind: Some(field("type", stamp(1, 0), Value::String("page".into()))),
        blocks: vec![
            block("root", stamp(2, 0), "", "V", "r"),
            block("orphan", stamp(2, 0), "missing", "V", "o"),
        ],
        messages: vec![message],
        ..Record::default()
    };
    let view = record_view(&record);
    assert_eq!(view["typeKey"], "page");
    assert_eq!(view["createdAt"], 1);
    assert_eq!(view["updatedAt"], 3);
    assert_eq!(view["blocks"][0]["childrenIds"], json!(["m"]));
    assert_eq!(view["blocks"][1]["content"], json!({"custom": {"contentType": "chat", "meta": {"text": "hi"}}}));
    assert_eq!(view["blocks"].as_array().unwrap().len(), 2);
}

#[test]
fn hlc_is_monotonic_under_a_backwards_clock() {
    let mut clock = Hlc::default();
    let mut last = 0;
    for wall in [1000, 1000, 999, 500, 1001, 0, 2000] {
        let time = hlc_now(&mut clock, wall);
        assert!(time > last, "{time} after {last}");
        last = time;
    }
    // Observing a newer stamp moves the clock past it.
    hlc_observe(&mut clock, stamp(5000, 3), 2000).unwrap();
    assert!(hlc_now(&mut clock, 2000) > stamp(5000, 3).time);
}

#[test]
fn far_future_stamps_are_refused() {
    let mut clock = Hlc::default();
    let day = 24 * 60 * 60 * 1000;
    let wall = 1_700_000_000_000;
    assert!(hlc_observe(&mut clock, stamp(wall + day, 0), wall).is_ok());
    let before = clock;
    assert!(hlc_observe(&mut clock, stamp(wall + day + 1, 0), wall).is_err());
    assert_eq!(clock, before);
}

#[test]
fn fractional_keys_sort_and_bisect() {
    let positions = sibling_positions(50);
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{positions:?}");
    let mut a = String::new();
    for _ in 0..100 {
        let next = key_between(&a, None);
        assert!(next > a);
        a = next;
    }
    let low = key_between("", None);
    let mut high = key_between(&low, None);
    for _ in 0..100 {
        let mid = key_between(&low, Some(&high));
        assert!(low < mid && mid < high, "{low} {mid} {high}");
        high = mid;
    }
}

// ── Through the engine: conversion and the shared vectors ────────────

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::executor::block_on;
use roostr_engine::record::{
    Patch as RecordPatch, bucket_hashes, from_object_state, record_hash, record_state, root_hash,
};
use roostr_engine::{Change, Engine};

fn engine() -> Engine {
    Engine::native().expect("native engine")
}

fn odin_file(path: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("GLON_ODIN_ROOT")).join(path);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Devices in first-seen author order; "" (a seed checkpoint) included.
fn device_table(changes: &[Change]) -> BTreeMap<String, u32> {
    let mut devices = BTreeMap::from([(String::new(), 0)]);
    for change in changes {
        let next = devices.len() as u32;
        devices.entry(change.author.clone()).or_insert(next);
    }
    devices
}

/// The replay state without the engine-derived projections the record
/// view does not produce.
fn comparable(mut state: serde_json::Value) -> serde_json::Value {
    if let Some(object) = state.as_object_mut() {
        object.remove("conversations");
        object.remove("mailbox");
        object.remove("heads");
    }
    state
}

/// A record gives every block one place, so a legacy graph that lists a
/// block under a parent and also emits it elsewhere loses that extra
/// reference: drop it from the replayed side the way conversion does (a
/// child reference to a block already placed earlier in preorder).
fn drop_repeated_refs(state: &mut serde_json::Value) -> bool {
    let Some(blocks) = state["blocks"].as_array_mut() else {
        return false;
    };
    let exists: std::collections::HashSet<String> = blocks
        .iter()
        .filter_map(|block| block["id"].as_str().map(str::to_owned))
        .collect();
    let mut placed = std::collections::HashSet::new();
    let mut changed = false;
    for block in blocks {
        placed.insert(block["id"].as_str().unwrap_or("").to_owned());
        let Some(kids) = block["childrenIds"].as_array_mut() else {
            continue;
        };
        kids.retain(|kid| {
            let kid = kid.as_str().unwrap_or("");
            if exists.contains(kid) && placed.contains(kid) {
                changed = true;
                return false;
            }
            placed.insert(kid.to_owned());
            true
        });
    }
    changed
}

/// For every replay fixture and conversion vector (participants.json's
/// included): view(convert(replay)) is today's replay state, and the Rust
/// conversion is byte-identical to the engine's own.
#[test]
fn converted_view_equals_replay() {
    let engine = engine();
    let mut checked = 0;
    let participants = vectors("participants")["conversion"].clone();
    let sources = [
        odin_file("core/replay_fixtures.json"),
        odin_file("core/testdata/record_vectors/conversion.json"),
        participants,
    ];
    for source in &sources {
        for fixture in source.as_array().expect("array") {
            let name = fixture["name"].as_str().unwrap_or("");
            let changes: Vec<Change> =
                serde_json::from_value(fixture["changes"].clone()).expect("changes");
            if changes.is_empty() {
                continue;
            }
            let devices = device_table(&changes);
            let record = block_on(engine.record_from_changes(&changes, None, |author| {
                devices.get(author).copied()
            }))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            let mut state = block_on(engine.replay(&changes, None)).unwrap().expect("state");
            let repeated = drop_repeated_refs(&mut state);
            assert!(
                !repeated || name.contains("legacy child root serialized once"),
                "{name}: only the legacy graph fixture repeats a block"
            );
            assert_eq!(comparable(record_view(&record)), comparable(state), "{name}: view");
            let theirs = block_on(engine.engine_record_from_changes(&changes, None, &devices))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(hex(&record_canonical(&record)), theirs.canonical, "{name}: canonical");
            assert_eq!(hex(&record_hash(&record)), theirs.hash, "{name}: hash");
            checked += 1;
        }
    }
    assert!(checked >= 20, "{checked} fixtures checked");
}

#[test]
fn unknown_author_is_refused() {
    let state = json!({"id": "o", "typeKey": "page", "fields": {}, "blocks": [], "deleted": false, "createdAt": 1, "updatedAt": 1});
    let writers = serde_json::from_value(json!({"type": {"time": 1, "author": "who"}})).unwrap();
    assert!(from_object_state(&state, &writers, |_| None).is_err());
    let record = from_object_state(&state, &writers, |_| Some(7)).unwrap();
    assert_eq!(record.kind.unwrap().stamp, stamp(1, 7));
}

fn vectors(name: &str) -> serde_json::Value {
    odin_file(&format!("core/testdata/record_vectors/{name}.json"))
}

fn expect_record(record: &Record, expected: &serde_json::Value, name: &str) {
    assert_eq!(record.to_json(), expected["record"], "{name}: record JSON");
    assert_eq!(hex(&record_canonical(record)), expected["canonical"], "{name}: canonical");
    assert_eq!(hex(&record_hash(record)), expected["hash"], "{name}: hash");
    let parsed = Record::from_json(&expected["record"]).expect("record JSON");
    assert_eq!(hex(&record_canonical(&parsed)), expected["canonical"], "{name}: reparsed");
}

#[test]
fn vectors_merge() {
    for case in vectors("merge").as_array().expect("cases") {
        let name = case["name"].as_str().unwrap_or("");
        let records: Vec<Record> = case["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| Record::from_json(record).expect("record"))
            .collect();
        let patches: Vec<RecordPatch> = case["patches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|patch| RecordPatch::from_json(patch).expect("patch"))
            .collect();
        let fold = |records: &[Record], patches: &[RecordPatch]| {
            let merged = records.iter().fold(Record::default(), |acc, record| {
                record_merge(&acc, record).unwrap_or_else(|error| panic!("{name}: {error}"))
            });
            patches.iter().fold(merged, |acc, patch| {
                record_apply_patch(&acc, patch).unwrap_or_else(|error| panic!("{name}: {error}"))
            })
        };
        let forward = fold(&records, &patches);
        expect_record(&forward, &case["expected"], name);
        let mut records_rev = records.clone();
        records_rev.reverse();
        let mut patches_rev = patches.clone();
        patches_rev.reverse();
        let backward = fold(&records_rev, &patches_rev);
        assert_eq!(record_canonical(&backward), record_canonical(&forward), "{name}: reversed");
    }
}

#[test]
fn vectors_view() {
    for case in vectors("view").as_array().expect("cases") {
        let name = case["name"].as_str().unwrap_or("");
        let record = Record::from_json(&case["record"]).expect("record");
        assert_eq!(record_state(&record), case["expected"], "{name}");
    }
}

#[test]
fn vectors_fingerprint() {
    let case = vectors("fingerprint");
    let records: Vec<Record> = case["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| Record::from_json(record).expect("record"))
        .collect();
    let expected = &case["expected"];
    for record in &records {
        assert_eq!(
            hex(&record_hash(record)),
            expected["hashes"][&record.object_id],
            "{}",
            record.object_id
        );
    }
    let buckets = bucket_hashes(&records);
    let empty = expected["emptyBucket"].as_str().unwrap();
    for (index, hash) in buckets.iter().enumerate() {
        let want = expected["buckets"][index.to_string()].as_str().unwrap_or(empty);
        assert_eq!(hex(hash), want, "bucket {index}");
    }
    assert_eq!(hex(&root_hash(&buckets)), expected["root"]);
}

#[test]
fn vectors_positions() {
    let case = vectors("positions");
    for count in case["counts"].as_array().unwrap() {
        let n = count.as_u64().unwrap() as usize;
        assert_eq!(json!(sibling_positions(n)), case["expected"][n.to_string()], "{n}");
    }
}

#[test]
fn vectors_hlc() {
    for case in vectors("hlc").as_array().expect("cases") {
        let name = case["name"].as_str().unwrap_or("");
        let mut clock = Hlc::default();
        let steps = case["steps"].as_array().unwrap();
        for (step, expected) in steps.iter().zip(case["expected"].as_array().unwrap()) {
            let wall = step["wall"].as_u64().unwrap();
            let got = match step["op"].as_str().unwrap() {
                "now" => json!({"time": hlc_now(&mut clock, wall)}),
                "observe" => {
                    let at = Stamp {
                        time: step["time"].as_u64().unwrap(),
                        device: step["device"].as_u64().unwrap_or(0) as u32,
                    };
                    let ok = hlc_observe(&mut clock, at, wall).is_ok();
                    json!({"ok": ok, "last": clock.last})
                }
                op => panic!("{name}: unknown op {op}"),
            };
            assert_eq!(&got, expected, "{name}: {step}");
        }
    }
}

/// Changes → record through one engine replay plus the Rust stamping.
#[test]
fn vectors_conversion() {
    let engine = engine();
    for case in vectors("conversion").as_array().expect("cases") {
        let name = case["name"].as_str().unwrap_or("");
        let changes: Vec<Change> =
            serde_json::from_value(case["changes"].clone()).expect("changes");
        let devices: BTreeMap<String, u32> =
            serde_json::from_value(case["devices"].clone()).expect("devices");
        let checkpoint = case["checkpoint"].as_str().map(|text| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(text).expect("checkpoint")
        });
        let result = block_on(engine.record_from_changes(&changes, checkpoint.as_deref(), |author| {
            devices.get(author).copied()
        }));
        match case["expected"].get("error") {
            Some(_) => assert!(result.is_err(), "{name}: expected an error"),
            None => {
                let record = result.unwrap_or_else(|error| panic!("{name}: {error}"));
                expect_record(&record, &case["expected"], name);
            }
        }
    }
}

/// participants.json: conversion splits a discussion's participants into
/// entries; the view joins them back and hides them.
#[test]
fn vectors_participants() {
    let engine = engine();
    let case = vectors("participants");
    for case in case["conversion"].as_array().expect("conversion") {
        let name = case["name"].as_str().unwrap_or("");
        let changes: Vec<Change> =
            serde_json::from_value(case["changes"].clone()).expect("changes");
        let devices: BTreeMap<String, u32> =
            serde_json::from_value(case["devices"].clone()).expect("devices");
        let record = block_on(engine.record_from_changes(&changes, None, |author| {
            devices.get(author).copied()
        }))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        expect_record(&record, &case["expected"], name);
    }
    for case in case["view"].as_array().expect("view") {
        let name = case["name"].as_str().unwrap_or("");
        let record = Record::from_json(&case["record"]).expect("record");
        assert_eq!(record_state(&record), case["expected"], "{name}");
    }
}
