//! Cold start from stored change bytes (`load_corpus`): the loaded cache
//! answers exactly like the JSON path, outlives the blob it was read from,
//! loads large vaults in batches without losing earlier ones or splitting an
//! object's history, counts damaged changes, and a refused load leaves the
//! host owing a reset.

#![cfg(not(target_family = "wasm"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::executor::block_on;
use roostr_engine::{Change, Engine, EngineError, ObjectHistory, QueryResult, QueryUpdate};
use serde_json::{Value, json};

/// The query cache is engine-global: tests that use it take turns.
static CACHE: Mutex<()> = Mutex::new(());

fn cache_turn() -> MutexGuard<'static, ()> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn engine() -> Engine {
    Engine::native().expect("native engine")
}

fn change_of(i: i64, name: &str) -> Change {
    serde_json::from_value(json!({
        "objectId": format!("obj-{i}"), "parentIds": [], "timestamp": i + 1, "author": "test",
        "ops": [
            {"objectCreate": {"typeKey": "note"}},
            {"fieldSet": {"key": "name", "value": {"stringValue": name}}},
            {"fieldSet": {"key": "done", "value": {"boolValue": i % 2 == 0}}},
        ],
    }))
    .unwrap()
}

/// `change` with its content address set.
fn addressed(engine: &Engine, mut change: Change) -> Change {
    change.id = block_on(engine.change_id(&change)).unwrap();
    change
}

fn bytes_of(engine: &Engine, change: &Change) -> Vec<u8> {
    block_on(engine.encode_change(change)).unwrap()
}

fn history(changes: Vec<Vec<u8>>) -> ObjectHistory {
    ObjectHistory {
        checkpoint: None,
        changes,
    }
}

fn state_of(engine: &Engine, change: Change) -> Value {
    let change = addressed(engine, change);
    block_on(engine.replay(&[change], None))
        .unwrap()
        .expect("a state")
}

/// Queries the cache without changing it.
fn cached(engine: &Engine, body: &Value) -> QueryResult {
    block_on(engine.query(QueryUpdate {
        upserts: &[],
        removed: &[],
        reset: false,
        body: Some(body),
        now_ms: 0,
    }))
    .expect("query")
}

fn sorted_names(result: &QueryResult) -> Vec<String> {
    let mut names: Vec<String> = result
        .records
        .iter()
        .map(|row| row["name"].as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

fn all() -> Value {
    json!({"filters": [], "limit": 500})
}

#[test]
fn a_vault_loaded_from_bytes_answers_like_one_loaded_from_json() {
    let _turn = cache_turn();
    let engine = engine();
    let changes: Vec<Change> = (0..40)
        .map(|i| change_of(i, &format!("Note {i}")))
        .collect();
    let histories: Vec<ObjectHistory> = changes
        .iter()
        .map(|change| history(vec![bytes_of(&engine, change)]))
        .collect();
    let load = block_on(engine.load_corpus(&histories, true, 0)).unwrap();
    assert_eq!((load.objects, load.changes, load.skipped), (40, 40, 0));
    let from_bytes = cached(&engine, &all());
    assert_eq!(from_bytes.total, 40);

    let states: Vec<Value> = changes
        .into_iter()
        .map(|change| state_of(&engine, change))
        .collect();
    let from_json = block_on(engine.query_objects(&states, &all(), 0)).unwrap();
    assert_eq!(from_json.total, 40);
    assert_eq!(sorted_names(&from_json), sorted_names(&from_bytes));
}

/// The codec borrows strings from the bytes it reads: a cached state must
/// not point at the request blob the next call overwrites.
#[test]
fn a_cached_object_survives_the_blob_being_reused() {
    let _turn = cache_turn();
    let engine = engine();
    let durable = history(vec![bytes_of(&engine, &change_of(900, "Durable name"))]);
    block_on(engine.load_corpus([&durable], true, 0)).unwrap();
    let next = history(vec![bytes_of(&engine, &change_of(901, "ZZZZZZZZ"))]);
    block_on(engine.load_corpus([&next], false, 0)).unwrap();
    assert_eq!(
        sorted_names(&cached(&engine, &json!({"filters": [], "limit": 10}))),
        ["Durable name", "ZZZZZZZZ"]
    );
}

/// 9,000 changes is past one batch: a later batch must not wipe an earlier one.
#[test]
fn a_big_vault_loads_in_batches() {
    let _turn = cache_turn();
    let engine = engine();
    let histories: Vec<ObjectHistory> = (0..9000)
        .map(|i| {
            history(vec![bytes_of(
                &engine,
                &change_of(i, &format!("Batch {i}")),
            )])
        })
        .collect();
    let load = block_on(engine.load_corpus(&histories, true, 0)).unwrap();
    assert_eq!(load.changes, 9000);
    assert_eq!(
        load.cached, 9000,
        "the engine's own count after the last batch"
    );
    assert_eq!(
        cached(&engine, &json!({"filters": [], "limit": 1})).total,
        9000
    );
}

#[test]
fn damaged_changes_are_counted_without_discarding_healthy_objects() {
    let _turn = cache_turn();
    let engine = engine();
    let histories = [
        history(vec![bytes_of(&engine, &change_of(1, "Good"))]),
        history(vec![vec![0xff, 0xff, 0xff]]),
        history(vec![bytes_of(&engine, &change_of(2, "Also good"))]),
    ];
    let load = block_on(engine.load_corpus(&histories, true, 0)).unwrap();
    assert_eq!(load.skipped, 1);
    assert_eq!(
        sorted_names(&cached(&engine, &all())),
        ["Also good", "Good"]
    );
}

/// An object's history placed after a full batch of others still replays
/// whole: its update lands on its creation.
#[test]
fn an_object_history_in_a_later_batch_replays_whole() {
    let _turn = cache_turn();
    let engine = engine();
    let creation = addressed(&engine, change_of(9000, "Original"));
    let update: Change = serde_json::from_value(json!({
        "objectId": creation.object_id, "parentIds": [creation.id],
        "timestamp": creation.timestamp + 1, "author": "test",
        "ops": [{"fieldSet": {"key": "name", "value": {"stringValue": "Updated"}}}],
    }))
    .unwrap();
    let mut histories: Vec<ObjectHistory> = (0..4001)
        .map(|i| {
            history(vec![bytes_of(
                &engine,
                &change_of(i, &format!("Filler {i}")),
            )])
        })
        .collect();
    histories.insert(
        4000,
        history(vec![
            bytes_of(&engine, &creation),
            bytes_of(&engine, &update),
        ]),
    );
    block_on(engine.load_corpus(&histories, true, 0)).unwrap();
    let rows = cached(
        &engine,
        &json!({"type": "note", "filters": [{"key": "id", "condition": "equal", "value": creation.object_id}]}),
    );
    assert_eq!(rows.total, 1);
    let row = &rows.records[0];
    assert_eq!(row["id"], json!(creation.object_id));
    assert_eq!(row["typeKey"], "note");
    assert_eq!(row["name"], "Updated");
    assert_eq!(row["createdAt"], json!(creation.timestamp));
    assert_eq!(row["fields"]["done"], json!({"boolValue": true}));
}

#[test]
fn one_long_object_history_is_never_split_at_a_change_count_boundary() {
    let _turn = cache_turn();
    let engine = engine();
    let creation = addressed(&engine, change_of(1, "Long history"));
    let mut changes = vec![bytes_of(&engine, &creation)];
    let mut parent = creation.id.clone();
    for i in 0..4001_i64 {
        let change: Change = serde_json::from_value(json!({
            "objectId": creation.object_id, "parentIds": [parent],
            "timestamp": creation.timestamp + i + 1, "author": "test",
            "ops": [{"fieldSet": {"key": "revision", "value": {"intValue": i}}}],
        }))
        .unwrap();
        parent = block_on(engine.change_id(&change)).unwrap();
        changes.push(bytes_of(&engine, &change));
    }
    block_on(engine.load_corpus([&history(changes)], true, 0)).unwrap();
    let rows = cached(&engine, &json!({"type": "note"}));
    assert_eq!(rows.total, 1);
    let row = &rows.records[0];
    assert_eq!(row["id"], json!(creation.object_id));
    assert_eq!(row["name"], "Long history");
    assert_eq!(row["createdAt"], json!(creation.timestamp));
    assert_eq!(row["fields"]["revision"], json!({"intValue": 4000}));
}

/// An object larger than a batch is refused rather than replayed as a
/// prefix; the batches before it stay applied, so the host's JSON fallback
/// must reset.
#[test]
fn a_refused_later_history_leaves_the_host_owing_a_reset() {
    let _turn = cache_turn();
    let engine = engine();
    let mut histories: Vec<ObjectHistory> = (0..4001)
        .map(|i| {
            history(vec![bytes_of(
                &engine,
                &change_of(i, &format!("Partial {i}")),
            )])
        })
        .collect();
    // Framing counts toward the byte limit too.
    histories.push(history(vec![vec![0; 24 * 1024 * 1024]]));
    let refused = block_on(engine.load_corpus(&histories, true, 0));
    assert!(
        matches!(refused, Err(EngineError::BlobTooLarge(_))),
        "an oversized object history is refused: {refused:?}"
    );
    assert_eq!(cached(&engine, &all()).total, 4000, "the first batch stays");

    let survivor = state_of(&engine, change_of(8000, "Authoritative snapshot"));
    let rows = block_on(engine.query(QueryUpdate {
        upserts: &[survivor],
        removed: &[],
        reset: true,
        body: Some(&all()),
        now_ms: 0,
    }))
    .unwrap();
    assert_eq!(sorted_names(&rows), ["Authoritative snapshot"]);
}
