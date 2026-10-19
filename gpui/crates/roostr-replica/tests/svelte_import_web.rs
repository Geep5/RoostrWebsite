//! The browser replica's one-time import of the Svelte site's IndexedDB
//! vault (`roostr_replica_store::svelte_import`), in a real browser: the
//! `roostr` database is seeded exactly as the Svelte app's
//! `engine/store.ts` (DB_VERSION 5) created and filled it, then the replica
//! boots through `ReplicaBackend::open_browser` as roostr.space does.
//!
//! Needs the site's engine at `ROOSTR_ENGINE_TEST_BASE` (build time; default
//! `http://127.0.0.1:8787/`: RoostrWebsite `static/` served with CORS) and a
//! wasm32 test runner driving a browser (`wasm-bindgen-test-runner`).

#![cfg(target_family = "wasm")]

use std::time::Duration;

use roostr_core::backend::Backend;
use roostr_engine::{Change, Engine, Op, StateInputs, WebSource};
use roostr_nostr::Keys;
use roostr_replica::ReplicaBackend;
use roostr_replica::local::{BrowserLocalKv, LocalKv};
use roostr_replica_store::svelte_import::{self, IMPORT_FLAG, SVELTE_DB, SvelteImport};
use roostr_replica_store::{ChangeStore, HeldItem, HeldQuery, IdbChangeStore};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
const DB_NAME = "roostr";
const DB_VERSION = 5;
const CHANGES = "changes";
const CHECKPOINTS = "checkpoints";
const META = "meta";
const STATES = "states";
const RELAY_EVENTS = "relay-events";
const BASES = "bases";
const BASE_CURRENT = "base-current";

function req(r) {
	return new Promise((resolve, reject) => {
		r.onsuccess = () => resolve(r.result);
		r.onerror = () => reject(r.error);
	});
}

function txDone(tx) {
	return new Promise((resolve, reject) => {
		tx.oncomplete = () => resolve();
		tx.onerror = () => reject(tx.error);
		tx.onabort = () => reject(tx.error ?? new Error("transaction aborted"));
	});
}

function hex(text) {
	return Uint8Array.from(text.match(/../g) ?? [], (byte) => parseInt(byte, 16));
}

// store.ts ChangeStore.open(), its upgrade verbatim.
function openStoreTs() {
	return new Promise((resolve, reject) => {
		const r = indexedDB.open(DB_NAME, DB_VERSION);
		r.onupgradeneeded = () => {
			const db = r.result;
			if (!db.objectStoreNames.contains(CHANGES)) {
				const store = db.createObjectStore(CHANGES);
				store.createIndex("objectId", "objectId", { unique: false });
			}
			if (!db.objectStoreNames.contains(CHECKPOINTS)) db.createObjectStore(CHECKPOINTS);
			if (!db.objectStoreNames.contains(META)) db.createObjectStore(META);
			if (!db.objectStoreNames.contains(STATES)) db.createObjectStore(STATES);
			if (!db.objectStoreNames.contains(RELAY_EVENTS)) {
				db.createObjectStore(RELAY_EVENTS, { keyPath: ["scope", "id"] }).createIndex("scopeKind", ["scope", "kind"], { unique: false });
			}
			const relayEvents = r.transaction.objectStore(RELAY_EVENTS);
			if (!relayEvents.indexNames.contains("scopeB")) relayEvents.createIndex("scopeB", ["scope", "b"], { unique: false });
			if (!db.objectStoreNames.contains(BASES)) db.createObjectStore(BASES, { keyPath: ["objectId", "hash"] });
			if (!db.objectStoreNames.contains(BASE_CURRENT)) db.createObjectStore(BASE_CURRENT);
		};
		r.onsuccess = () => resolve(r.result);
		r.onerror = () => reject(r.error);
	});
}

export async function resetOrigin() {
	localStorage.clear();
	for (const name of [DB_NAME, "roostr-replica", "svelte-import-probe"]) {
		await new Promise((resolve) => {
			const r = indexedDB.deleteDatabase(name);
			r.onsuccess = r.onerror = () => resolve();
		});
	}
}

// Every write below is the one the store.ts method named beside it makes.
export async function seedStoreTs(text) {
	const v = JSON.parse(text);
	const db = await openStoreTs();
	// addChanges: relay imports.
	let tx = db.transaction(CHANGES, "readwrite");
	for (const c of v.imported) {
		const row = { objectId: c.json.objectId, bytes: hex(c.bytes), json: c.json };
		// addRebased marks the orphan: `{...orphan, rebased: <rebased change id>}`.
		if (c.rebased) row.rebased = c.rebased;
		tx.objectStore(CHANGES).put(row, c.json.id);
	}
	await txDone(tx);
	// addLocalChange: the change and its publication obligation, atomically.
	for (const c of v.local) {
		tx = db.transaction([CHANGES, META], "readwrite");
		tx.objectStore(CHANGES).put({ objectId: c.json.objectId, bytes: hex(c.bytes), json: c.json }, c.json.id);
		const pending = { key: c.json.id, changeId: c.json.id, objectId: c.json.objectId, bytes: hex(c.bytes) };
		tx.objectStore(META).put(pending, `pending:${c.json.id}`);
		await txDone(tx);
	}
	// markPublished.
	for (const id of v.published) {
		tx = db.transaction(META, "readwrite");
		tx.objectStore(META).put(true, `published:${id}`);
		tx.objectStore(META).delete(`pending:${id}`);
		await txDone(tx);
	}
	// putBase: the base, the current pointer and the stream floors.
	for (const b of v.bases) {
		tx = db.transaction([BASES, BASE_CURRENT, META], "readwrite");
		const row = { objectId: b.objectId, hash: b.hash, epoch: b.epoch, createdAt: b.createdAt, prevBase: b.prevBase, bytes: hex(b.bytes) };
		const byStream = { ...((await req(tx.objectStore(META).get("base-streams"))) ?? {}) };
		for (const scope of b.streams) byStream[scope] = row.createdAt;
		tx.objectStore(META).put(byStream, "base-streams");
		tx.objectStore(BASES).put(row);
		tx.objectStore(BASE_CURRENT).put({ hash: row.hash, createdAt: row.createdAt }, row.objectId);
		await txDone(tx);
	}
	// putCheckpoint.
	for (const cp of v.checkpoints) {
		tx = db.transaction(CHECKPOINTS, "readwrite");
		tx.objectStore(CHECKPOINTS).put({ objectId: cp.objectId, bytes: hex(cp.bytes), hash: cp.hash, heads: cp.heads }, cp.objectId);
		await txDone(tx);
	}
	// recordRelayEvents.
	tx = db.transaction(RELAY_EVENTS, "readwrite");
	for (const e of v.relayEvents) {
		const held = { id: e.id, createdAt: e.createdAt, kind: e.kind, scope: e.scope };
		if (e.b) held.b = e.b;
		tx.objectStore(RELAY_EVENTS).put(held);
	}
	await txDone(tx);
	// putState: the replay memo.
	for (const [objectId, state] of Object.entries(v.states)) {
		tx = db.transaction(STATES, "readwrite");
		tx.objectStore(STATES).put({ n: 1, cp: "", base: "", state }, objectId);
		await txDone(tx);
	}
	// setCursor (with replay groups), setBootstrapped.
	if (v.cursor !== undefined) {
		tx = db.transaction(META, "readwrite");
		tx.objectStore(META).put(v.cursor, "cursor");
		tx.objectStore(META).put(v.replayGroups, "replay-groups");
		await txDone(tx);
	}
	if (v.bootstrapped) {
		tx = db.transaction(META, "readwrite");
		tx.objectStore(META).put(true, "bootstrapped");
		await txDone(tx);
	}
	// setBootstrapFloor: an interrupted first walk.
	if (v.bootstrapFloor !== undefined) {
		tx = db.transaction(META, "readwrite");
		tx.objectStore(META).put(v.bootstrapFloor, "bootstrap-floor");
		await txDone(tx);
	}
	db.close();
}

// What the Svelte database holds now (it must exist).
export async function storeTsRows() {
	const db = await openStoreTs();
	const tx = db.transaction([CHANGES, META], "readonly");
	const changes = await req(tx.objectStore(CHANGES).getAllKeys());
	const meta = await req(tx.objectStore(META).getAllKeys());
	const stamp = (await req(tx.objectStore(META).get("gpui-import"))) ?? null;
	db.close();
	return JSON.stringify({ changes, meta, stamp });
}

export async function databaseNames() {
	return JSON.stringify((await indexedDB.databases()).map((db) => db.name));
}

// A browser without indexedDB.databases() (older Firefox and Safari).
export function hideDatabases(hidden) {
	if (hidden) indexedDB.databases = undefined;
	else delete indexedDB.databases;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = resetOrigin)]
    async fn reset_origin();
    #[wasm_bindgen(js_name = seedStoreTs, catch)]
    async fn seed_store_ts(vault: String) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = storeTsRows, catch)]
    async fn store_ts_rows() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = databaseNames)]
    async fn database_names() -> JsValue;
    #[wasm_bindgen(js_name = hideDatabases)]
    fn hide_databases(hidden: bool);
}

/// A port nothing listens on: every publish attempt fails, so the
/// obligation stays observable in the store.
const UNREACHABLE_RELAYS: &str = r#"["ws://127.0.0.1:59999"]"#;
const CURSOR: i64 = 1_700_000_000;
const BASE_MS: i64 = 1_700_000_000_000;

fn source() -> WebSource {
    WebSource::under(option_env!("ROOSTR_ENGINE_TEST_BASE").unwrap_or("http://127.0.0.1:8787/"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sign_in(keys: &Keys) {
    BrowserLocalKv.set("roostr-key", Some(&keys.secret_hex()));
    BrowserLocalKv.set("roostr-relays", Some(UNREACHABLE_RELAYS));
}

/// A change creating `object_id` named `name`, addressed and encoded:
/// (the codec's decoded change - store.ts' `json` - and its bytes).
async fn change(
    engine: &Engine,
    author: &str,
    object_id: &str,
    name: &str,
    timestamp: i64,
) -> (Change, Vec<u8>) {
    let mut change = Change {
        id: String::new(),
        object_id: object_id.into(),
        parent_ids: Vec::new(),
        ops: vec![
            Op::ObjectCreate {
                type_key: "page".into(),
            },
            Op::FieldSet {
                key: "name".into(),
                value: Some(json!({"stringValue": name})),
            },
        ],
        timestamp,
        author: author.into(),
        snapshot: None,
        b: None,
    };
    change.id = engine.change_id(&change).await.unwrap();
    let bytes = engine.encode_change(&change).await.unwrap();
    (engine.decode_change(&bytes).await.unwrap(), bytes)
}

fn row(change: &Change, bytes: &[u8]) -> Value {
    json!({"json": change, "bytes": hex(bytes)})
}

async fn names() -> Vec<String> {
    serde_json::from_str(&database_names().await.as_string().unwrap()).unwrap()
}

async fn svelte_rows() -> Value {
    serde_json::from_str(&store_ts_rows().await.unwrap().as_string().unwrap()).unwrap()
}

#[wasm_bindgen_test]
async fn a_svelte_vault_is_imported_once_and_its_pending_change_published() {
    reset_origin().await;
    let keys = Keys::generate();
    sign_in(&keys);
    let author = keys.author_id();
    let engine = Engine::load(&source())
        .await
        .expect("engine at ROOSTR_ENGINE_TEST_BASE");

    // Written in the Svelte app and never published.
    let (local, local_bytes) =
        change(&engine, &author, "svelte-page", "Unpublished page", 2_000).await;
    // Published, then compacted into a base.
    let (synced, synced_bytes) =
        change(&engine, &author, "svelte-note", "Synced note", 1_000).await;
    let plan = engine
        .compact_plan(
            &StateInputs {
                bases: Vec::new(),
                changes: vec![synced.clone()],
                checkpoint: None,
                pending: Vec::new(),
            },
            BASE_MS,
            true,
        )
        .await
        .unwrap();
    assert!(plan.compact, "{}", plan.reason);
    let base = plan.base.unwrap();
    let decoded = engine.base_decode(&base).await.unwrap();
    let base_hash = decoded.summary.hash.clone();
    assert_eq!(decoded.covered_ids, vec![synced.id.clone()]);

    let vault = json!({
        "imported": [row(&synced, &synced_bytes)],
        "local": [row(&local, &local_bytes)],
        "published": [synced.id],
        "bases": [{
            "objectId": synced.object_id,
            "hash": base_hash,
            "epoch": decoded.summary.epoch,
            "createdAt": decoded.summary.created_at,
            "prevBase": decoded.summary.prev_base,
            "bytes": hex(&base),
            "streams": [""],
        }],
        // A legacy checkpoint row (the same protobuf message) for the same object.
        "checkpoints": [{
            "objectId": synced.object_id,
            "bytes": hex(&base),
            "hash": "c0ffee",
            "heads": [synced.id],
        }],
        "relayEvents": [
            {"id": "e".repeat(64), "createdAt": CURSOR - 10, "kind": 1078, "scope": ""},
            {"id": "f".repeat(64), "createdAt": CURSOR, "kind": 1078, "scope": "", "b": base_hash},
        ],
        "states": {(synced.object_id.clone()): {"id": synced.object_id, "typeKey": "page"}},
        "cursor": CURSOR,
        "replayGroups": [],
        "bootstrapped": true,
    });
    seed_store_ts(vault.to_string())
        .await
        .expect("seed store.ts vault");

    let backend = ReplicaBackend::open_browser(&source()).await.unwrap();
    backend.start().await.unwrap();

    let store = IdbChangeStore::open(IdbChangeStore::DEFAULT_NAME)
        .await
        .unwrap();
    assert_eq!(
        store.meta(IMPORT_FLAG.into()).await.unwrap().as_deref(),
        Some("imported")
    );
    // The changes, bytes verbatim and JSON as the engine reads it.
    let held = store.changes_for(local.object_id.clone()).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, local.id);
    assert_eq!(held[0].bytes, local_bytes);
    assert!(!held[0].rebased);
    assert_eq!(
        serde_json::from_str::<Change>(&held[0].json).unwrap(),
        local
    );
    let held = store.changes_for(synced.object_id.clone()).await.unwrap();
    assert_eq!(
        (held.len(), held[0].bytes.as_slice()),
        (1, synced_bytes.as_slice())
    );
    // Publications, bases, the checkpoint, held events and the walk's progress.
    assert!(store.is_published(synced.id.clone()).await.unwrap());
    let bases = store.bases(synced.object_id.clone()).await.unwrap();
    assert_eq!(
        (bases.len(), bases[0].bytes.as_slice()),
        (1, base.as_slice())
    );
    assert_eq!(
        store
            .current_base(synced.object_id.clone())
            .await
            .unwrap()
            .unwrap()
            .hash,
        base_hash
    );
    assert_eq!(
        store.newest_base_by_stream().await.unwrap().get(""),
        Some(&decoded.summary.created_at)
    );
    let checkpoint = store
        .checkpoint(synced.object_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            checkpoint.hash.as_str(),
            checkpoint.covered,
            checkpoint.heads
        ),
        ("c0ffee", 1, vec![synced.id.clone()])
    );
    let events = store
        .held_events(
            String::new(),
            HeldQuery {
                kinds: vec![1078],
                ..HeldQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        events,
        vec![
            HeldItem {
                created_at: CURSOR - 10,
                id: "e".repeat(64)
            },
            HeldItem {
                created_at: CURSOR,
                id: "f".repeat(64)
            },
        ]
    );
    assert_eq!(store.cursor().await.unwrap(), CURSOR);
    assert!(store.replay_groups().await.unwrap().is_empty());
    assert!(store.bootstrapped().await.unwrap());

    // The replica serves the imported object.
    let object = backend.object(&local.object_id).await.unwrap();
    assert_eq!(object.name(), "Unpublished page");

    // The outbox picked the obligation up: its first attempt seals the change
    // and persists the signed events before sending them (to a relay that
    // never answers, so the obligation stays).
    let mut sealed = None;
    for _ in 0..150 {
        let pending = store.pending(local.id.clone()).await.unwrap();
        let pending = pending.expect("the imported obligation is held until published");
        if pending.events.is_some() {
            sealed = pending.events;
            break;
        }
        roostr_nostr::sleep(Duration::from_millis(100)).await;
    }
    let sealed = sealed.expect("the outbox attempted to publish the imported change");
    assert!(!sealed.is_empty());
    for event in &sealed {
        assert_eq!(event["kind"], 1078);
        assert_eq!(event["pubkey"].as_str(), Some(keys.pubkey()));
    }
    assert!(!store.is_published(local.id.clone()).await.unwrap());
    backend.stop();

    // The Svelte database is left in place, stamped as imported.
    let rows = svelte_rows().await;
    let changes = rows["changes"].as_array().unwrap();
    assert!(changes.contains(&json!(local.id)) && changes.contains(&json!(synced.id)));
    assert!(
        rows["meta"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("pending:{}", local.id)))
    );
    assert_eq!(rows["stamp"]["owner"].as_str(), Some(keys.pubkey()));
    assert!(rows["stamp"]["importedAt"].is_number());

    // Once: the flag settles every later start.
    assert_eq!(
        svelte_import::import_svelte_vault(&store, SVELTE_DB, &engine, keys.pubkey(), &author)
            .await
            .unwrap(),
        SvelteImport::Settled("imported".into())
    );
    // A store erased by a logout does not take the stamped vault again.
    let erased = IdbChangeStore::open("svelte-import-probe").await.unwrap();
    assert_eq!(
        svelte_import::import_svelte_vault(&erased, SVELTE_DB, &engine, keys.pubkey(), &author)
            .await
            .unwrap(),
        SvelteImport::Stamped
    );
    assert!(
        erased
            .changes_for(local.object_id.clone())
            .await
            .unwrap()
            .is_empty()
    );
    erased.close().await.unwrap();
    store.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn a_fresh_visitor_never_creates_the_svelte_database() {
    reset_origin().await;
    let keys = Keys::generate();
    sign_in(&keys);
    let backend = ReplicaBackend::open_browser(&source()).await.unwrap();
    backend.start().await.unwrap();
    backend.stop();
    assert!(!names().await.contains(&SVELTE_DB.to_string()));
    let store = IdbChangeStore::open(IdbChangeStore::DEFAULT_NAME)
        .await
        .unwrap();
    assert_eq!(
        store.meta(IMPORT_FLAG.into()).await.unwrap().as_deref(),
        Some("absent")
    );
    store.close().await.unwrap();

    // Without indexedDB.databases(): the probing open aborts its upgrade.
    let engine = Engine::load(&source()).await.unwrap();
    let probe = IdbChangeStore::open("svelte-import-probe").await.unwrap();
    hide_databases(true);
    let outcome = svelte_import::import_svelte_vault(
        &probe,
        SVELTE_DB,
        &engine,
        keys.pubkey(),
        &keys.author_id(),
    )
    .await;
    hide_databases(false);
    assert_eq!(outcome.unwrap(), SvelteImport::Absent);
    assert!(!names().await.contains(&SVELTE_DB.to_string()));
    probe.close().await.unwrap();
}

/// What a store gets straight from the import, before any sync touches it:
/// an interrupted first walk resumes, an orphan stays rebased.
#[wasm_bindgen_test]
async fn the_walk_progress_and_rebased_orphans_carry_over() {
    reset_origin().await;
    let engine = Engine::load(&source()).await.unwrap();
    let keys = Keys::generate();
    let author = keys.author_id();
    let (orphan, orphan_bytes) = change(&engine, &author, "svelte-page", "Orphan", 1_000).await;
    let (rebased, rebased_bytes) = change(&engine, &author, "svelte-page", "Rebased", 1_001).await;
    let mut orphan_row = row(&orphan, &orphan_bytes);
    orphan_row["rebased"] = json!(rebased.id);
    let vault = json!({
        "imported": [orphan_row, row(&rebased, &rebased_bytes)],
        "local": [],
        "published": [orphan.id, rebased.id],
        "bases": [],
        "checkpoints": [],
        "relayEvents": [],
        "states": {},
        "cursor": CURSOR,
        "replayGroups": [["group", CURSOR - 5]],
        "bootstrapFloor": CURSOR - 100,
    });
    seed_store_ts(vault.to_string())
        .await
        .expect("seed store.ts vault");

    let target = IdbChangeStore::open("svelte-import-probe").await.unwrap();
    let outcome =
        svelte_import::import_svelte_vault(&target, SVELTE_DB, &engine, keys.pubkey(), &author)
            .await
            .unwrap();
    let SvelteImport::Imported(stats) = outcome else {
        panic!("not imported: {outcome:?}");
    };
    assert_eq!(
        (
            stats.changes,
            stats.published,
            stats.pending,
            stats.skipped,
            stats.resumed
        ),
        (2, 2, 0, 0, true)
    );
    let held = target.changes_for(orphan.object_id.clone()).await.unwrap();
    let flags: Vec<(&str, bool)> = held.iter().map(|c| (c.id.as_str(), c.rebased)).collect();
    // Inserted in timestamp order; only the orphan is rebased.
    assert_eq!(
        flags,
        vec![(orphan.id.as_str(), true), (rebased.id.as_str(), false)]
    );
    let inputs = target
        .object_inputs(orphan.object_id.clone())
        .await
        .unwrap();
    assert_eq!(inputs.changes.len(), 1);
    assert_eq!(target.cursor().await.unwrap(), CURSOR);
    assert_eq!(
        target.replay_groups().await.unwrap(),
        vec![("group".to_string(), CURSOR - 5)]
    );
    assert_eq!(target.bootstrap_floor().await.unwrap(), Some(CURSOR - 100));
    assert!(!target.bootstrapped().await.unwrap());
    assert_eq!(
        target.meta(IMPORT_FLAG.into()).await.unwrap().as_deref(),
        Some("imported")
    );
    target.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn another_identitys_vault_is_left_alone() {
    reset_origin().await;
    let engine = Engine::load(&source()).await.unwrap();
    let other = Keys::generate();
    let (theirs, bytes) = change(
        &engine,
        &other.author_id(),
        "their-page",
        "Not yours",
        1_000,
    )
    .await;
    let vault = json!({
        "imported": [],
        "local": [row(&theirs, &bytes)],
        "published": [],
        "bases": [],
        "checkpoints": [],
        "relayEvents": [],
        "states": {},
    });
    seed_store_ts(vault.to_string())
        .await
        .expect("seed store.ts vault");
    let keys = Keys::generate();
    sign_in(&keys);

    let backend = ReplicaBackend::open_browser(&source()).await.unwrap();
    backend.start().await.unwrap();
    backend.stop();
    let store = IdbChangeStore::open(IdbChangeStore::DEFAULT_NAME)
        .await
        .unwrap();
    assert_eq!(
        store.meta(IMPORT_FLAG.into()).await.unwrap().as_deref(),
        Some("foreign")
    );
    assert!(
        store
            .changes_for(theirs.object_id.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.pending_publishes().await.unwrap().is_empty());
    store.close().await.unwrap();
    let rows = svelte_rows().await;
    assert!(rows["stamp"].is_null(), "not claimed: {rows}");
    assert!(
        rows["changes"]
            .as_array()
            .unwrap()
            .contains(&json!(theirs.id))
    );
}
