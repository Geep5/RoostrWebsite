//! The query engine through the native binding: glonOdin's
//! `fixtures/query-parity.json` cases (filters, dates, text, sets, sorts and
//! paging), answered alike over explicit objects and the resident cache; the
//! cache's delta transitions; and the scaling contract - a cold load past the
//! request reservation still queries, a steady-state query carries only the
//! delta, and the cache holds compact typed state.

#![cfg(not(target_family = "wasm"))]

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::executor::block_on;
use roostr_engine::{Change, Engine, EngineError, QueryResult, QueryUpdate, REQUEST_LIMIT};
use serde_json::{Value, json};

/// The query cache is engine-global: tests that use it take turns.
static CACHE: Mutex<()> = Mutex::new(());

fn cache_turn() -> MutexGuard<'static, ()> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn engine() -> Engine {
    Engine::native().expect("native engine")
}

fn query_fixture() -> Value {
    let path = PathBuf::from(env!("GLON_ODIN_ROOT"))
        .join("fixtures")
        .join("query-parity.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

/// Applies `upserts` / `removed` to the cache and runs `body`.
fn push(
    engine: &Engine,
    upserts: &[Value],
    removed: &[String],
    reset: bool,
    body: &Value,
    now_ms: i64,
) -> QueryResult {
    block_on(engine.query(QueryUpdate {
        upserts,
        removed,
        reset,
        body: Some(body),
        now_ms,
    }))
    .expect("query")
}

fn ids(result: &QueryResult) -> Vec<&str> {
    result
        .records
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default())
        .collect()
}

fn name_of(row: &Value) -> &str {
    row["name"].as_str().unwrap_or_default()
}

/// query-parity.json: every case's ids, total and snippet, over explicit
/// objects and through the cache (`parity-query-fixtures.ts`).
#[test]
fn query_fixtures() {
    let _turn = cache_turn();
    let engine = engine();
    let fixture = query_fixture();
    let now_ms = fixture["nowMs"].as_i64().expect("nowMs");
    let objects = fixture["objects"].as_array().expect("objects");
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let body = &case["body"];
        let direct = block_on(engine.query_objects(objects, body, now_ms))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(json!(ids(&direct)), case["ids"], "{name}: ids");
        assert_eq!(json!(direct.total), case["total"], "{name}: total");
        if let Some(snippet) = case["snippet"].as_str() {
            assert_eq!(
                direct.records.first().map(|row| &row["snippet"]),
                Some(&json!(snippet)),
                "{name}: snippet"
            );
        }
        assert_eq!(
            push(&engine, objects, &[], true, body, now_ms),
            direct,
            "{name}: through the cache"
        );
    }
}

/// The cache follows the declared deltas: an upsert replaces, a removal
/// drops, a reset replaces everything, the last duplicate of one push wins
/// and a full reset releases it (`parity-query-fixtures.ts`).
#[test]
fn cache_transitions() {
    let _turn = cache_turn();
    let engine = engine();
    let fixture = query_fixture();
    let objects = fixture["objects"].as_array().expect("objects");
    let body = json!({"type": "page", "sorts": [{"key": "id", "type": "asc"}]});
    let a = objects
        .iter()
        .find(|object| object["id"] == "a")
        .expect("a")
        .clone();

    let before = push(&engine, objects, &[], true, &body, 0);
    assert_eq!(name_of(&before.records[0]), "Alpha");

    let changed = "Changed in place without touching updatedAt";
    let mut edited = a.clone();
    edited["fields"]["name"] = json!({"stringValue": changed});
    let after = push(&engine, std::slice::from_ref(&edited), &[], false, &body, 0);
    assert_eq!(name_of(&after.records[0]), changed, "an upsert replaces");

    let without_a = push(&engine, &[], &["a".to_string()], false, &body, 0);
    assert_eq!(ids(&without_a), ["b", "c", "d"], "a removal drops");

    let only_a = push(&engine, std::slice::from_ref(&edited), &[], true, &body, 0);
    assert_eq!(
        (only_a.total, name_of(&only_a.records[0])),
        (1, changed),
        "a reset replaces the cache"
    );
    let empty = push(&engine, &[], &[], true, &body, 0);
    assert_eq!((empty.total, empty.records.len()), (0, 0));

    let mut duplicate = a.clone();
    duplicate["fields"] = json!({"name": {"stringValue": "Last duplicate wins"}});
    let duplicated = push(&engine, &[edited.clone(), duplicate], &[], true, &body, 0);
    assert_eq!(
        (duplicated.total, name_of(&duplicated.records[0])),
        (1, "Last duplicate wins")
    );

    let generation = engine.generation();
    block_on(engine.reset_all());
    assert_eq!(
        engine.generation(),
        generation + 1,
        "the host learns it must reload"
    );
    assert_eq!(push(&engine, &[], &[], false, &body, 0).total, 0);
    let rehydrated = push(&engine, std::slice::from_ref(&edited), &[], true, &body, 0);
    assert_eq!(name_of(&rehydrated.records[0]), changed);
}

#[test]
fn malformed_query_requests_are_refused() {
    let _turn = cache_turn();
    let engine = engine();
    for payload in [
        json!({"objects": [], "body": {}}),
        json!({"objects": false, "body": {}, "nowMs": 0}),
        json!({"upserts": [], "removed": [42], "body": {}, "nowMs": 0}),
        json!({"objects": [], "body": {}, "nowMs": 1e100}),
        json!({"objects": [], "body": {"limit": 1e100}, "nowMs": 0}),
    ] {
        let result = block_on(engine.call("query", payload.clone(), None));
        assert!(
            matches!(result, Err(EngineError::Domain(_))),
            "{payload}: expected a domain error, got {result:?}"
        );
    }
}

/// `vault(n, body_chars, prefix)` of `query-scale.test.ts`: alternating
/// task/note objects with a description of `body_chars` characters.
fn vault(n: usize, body_chars: usize, prefix: &str) -> Vec<Value> {
    let description = "x".repeat(body_chars);
    (0..n)
        .map(|i| {
            json!({
                "id": format!("{prefix}-{i}"),
                "typeKey": if i % 2 == 0 { "task" } else { "note" },
                "fields": {
                    "channel": {"stringValue": "space"},
                    "name": {"stringValue": format!("Object {i}")},
                    "description": {"stringValue": description},
                },
                "blocks": [],
                "deleted": false,
                "createdAt": 1_700_000_000_000_i64 + i as i64,
                "updatedAt": 1_700_000_000_000_i64 + i as i64,
            })
        })
        .collect()
}

fn tasks_body() -> Value {
    json!({
        "filters": [{"key": "typeKey", "condition": "equal", "value": "task"}],
        "sorts": [{"key": "updatedAt", "desc": true}],
        "limit": 10,
    })
}

fn json_len(objects: &[Value]) -> usize {
    objects
        .iter()
        .map(|object| serde_json::to_string(object).unwrap().len())
        .sum()
}

/// A cold start larger than the 16 MiB request reservation still queries.
#[test]
fn a_cold_start_past_the_request_reservation_still_queries() {
    let _turn = cache_turn();
    let engine = engine();
    let objects = vault(20_000, 700, "cold");
    assert!(json_len(&objects) > REQUEST_LIMIT);
    let result = push(&engine, &objects, &[], true, &tasks_body(), 0);
    assert_eq!((result.total, result.records.len()), (10_000, 10));
}

/// A steady-state query applies only what the host declares changed.
#[test]
fn a_steady_state_query_applies_only_the_declared_delta() {
    let _turn = cache_turn();
    let engine = engine();
    let body = tasks_body();
    let mut objects = vault(500, 100, "delta");
    assert_eq!(push(&engine, &objects, &[], true, &body, 0).total, 250);

    objects[0]["fields"]["name"] = json!({"stringValue": "renamed"});
    assert_eq!(
        push(&engine, &objects[..1], &[], false, &body, 0).total,
        250
    );
    let renamed = json!({"filters": [{"key": "name", "condition": "equal", "value": "renamed"}]});
    assert_eq!(
        ids(&push(&engine, &[], &[], false, &renamed, 0)),
        ["delta-0"]
    );

    let removed = push(&engine, &[], &["delta-0".to_string()], false, &body, 0);
    assert_eq!(removed.total, 249, "a declared removal drops the row");
}

/// `cacheBytes` as the engine reports it, from a one-change corpus push
/// (without a reset) that adds a probe object.
fn cache_bytes(engine: &Engine) -> u64 {
    let change: Change = serde_json::from_value(json!({
        "objectId": "cache-probe", "parentIds": [], "timestamp": 1, "author": "probe",
        "ops": [{"objectCreate": {"typeKey": "note"}}],
    }))
    .unwrap();
    let bytes = block_on(engine.encode_change(&change)).unwrap();
    let mut blob = (bytes.len() as u32).to_le_bytes().to_vec();
    blob.extend_from_slice(&bytes);
    let pushed = block_on(engine.call(
        "corpus",
        json!({"action": "push", "reset": false}),
        Some(&blob),
    ))
    .expect("probe push");
    pushed["cacheBytes"].as_u64().expect("cacheBytes")
}

/// ~12 MB of block-heavy state JSON: 1,500 notes of 90 small text blocks,
/// and its serialised size.
fn block_heavy_vault() -> (Vec<Value>, u64) {
    let objects: Vec<Value> = (0..1_500)
        .map(|i| {
            let blocks: Vec<Value> = (0..90)
                .map(|b| {
                    json!({
                        "id": format!("doc-{i}-{b}"),
                        "content": {"text": {"text": format!("line {b} of doc {i}")}},
                        "childrenIds": [],
                    })
                })
                .collect();
            json!({
                "id": format!("doc-{i}"), "typeKey": "note",
                "fields": {"name": {"stringValue": format!("Doc {i}")}},
                "blocks": blocks, "deleted": false, "createdAt": 1, "updatedAt": 1 + i,
            })
        })
        .collect();
    let payload = json_len(&objects) as u64;
    assert!(payload > 10 * 1024 * 1024);
    (objects, payload)
}

/// A cold load of `objects` into a released cache; its `cacheBytes`.
fn cold_load(engine: &Engine, objects: &[Value]) -> u64 {
    block_on(engine.reset_all());
    let search = json!({"filters": [], "limit": 1, "textQuery": "line 89 of doc 1499"});
    assert_eq!(push(engine, objects, &[], true, &search, 0).total, 1);
    cache_bytes(engine)
}

/// `size_of(core.Block)` (core/proto.odin) on engine.wasm's `js_wasm32` and
/// on the 64-bit native targets: its strings (16 vs 8 bytes), dynamic
/// arrays (40 vs 20) and slices double with the pointer width.
const BLOCK_BYTES_WASM32: u64 = 192;
const BLOCK_BYTES_64: u64 = 312;

/// A block-heavy vault caches compact typed state rather than its parsed
/// JSON tree (which put it near 14x the JSON).
///
/// `cacheBytes` counts the typed model, which typed `Block`s dominate, so
/// the bound scales with pointer width. query-scale.test.ts pinned 5x
/// against engine.wasm (wasm32), where this vault measures 3.69x (43,773,544
/// cache bytes for 11,852,373 JSON bytes, same core sources); natively it is
/// 5.75x (68,133,600). The 24.36 MB gap is the layout: each object's blocks
/// buffer grows to 128 slots of 312 instead of 192 bytes (23.04 MB over
/// 1,500 objects) and its 8-slot fields buffer holds 232- instead of
/// 128-byte `Value_Entry`s (1.25 MB). The 64-bit bound is that 5x scaled by
/// the `Block` size ratio (~8.1x): the same headroom over the measurement
/// the wasm bound has, far below the parsed-tree regression it guards.
#[test]
fn a_block_heavy_vault_caches_compact_state() {
    let _turn = cache_turn();
    let engine = engine();
    let (objects, payload) = block_heavy_vault();
    let cached = cold_load(&engine, &objects);
    block_on(engine.reset_all());
    assert!(
        cached * BLOCK_BYTES_WASM32 < 5 * payload * BLOCK_BYTES_64,
        "{cached} cache bytes for {payload} JSON bytes"
    );
}

/// A second cold load over the live cache (the "Retry opening vault" path)
/// never holds two snapshots.
#[test]
fn a_reset_over_a_live_cache_never_holds_two_snapshots() {
    let _turn = cache_turn();
    let engine = engine();
    let (objects, _) = block_heavy_vault();
    let cached = cold_load(&engine, &objects);
    let retried = push(&engine, &objects, &[], true, &json!({"limit": 1}), 1);
    assert_eq!(retried.total, 1_500);
    let again = cache_bytes(&engine);
    block_on(engine.reset_all());
    assert!(
        (again as f64) < cached as f64 * 1.1,
        "{again} cache bytes after the retry, {cached} before"
    );
}
