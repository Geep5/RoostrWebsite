//! The replica's state, vanish and query-cache bookkeeping, seen from the
//! inside: fixtures install a session without starting sync (no history
//! pass, so no catalog seeding) and read the private sets the TS tests read
//! off `WebBackend`. The native engine is one per process: tests take turns.

use std::time::{Duration, Instant};

use roostr_engine::Op;
use roostr_replica_store::SqliteChangeStore;

use super::*;
use crate::local::InMemoryLocalKv;

static TURN: Mutex<()> = Mutex::new(());

const WAIT: Duration = Duration::from_secs(30);

fn owner_keys() -> Keys {
    Keys::from_secret_hex(&hex::encode([1u8; 32])).unwrap()
}

fn member_keys() -> Keys {
    Keys::from_secret_hex(&hex::encode([2u8; 32])).unwrap()
}

fn key_hex() -> String {
    hex::encode([4u8; 32])
}

fn obj(value: Json) -> JsonMap {
    value.as_object().cloned().unwrap()
}

/// A change creating `object_id` with `fields`, at `timestamp`.
fn created(object_id: &str, type_key: &str, fields: &[(&str, Json)], timestamp: i64) -> Change {
    let mut ops = vec![Op::ObjectCreate {
        type_key: type_key.into(),
    }];
    ops.extend(fields.iter().map(|(key, value)| Op::FieldSet {
        key: (*key).into(),
        value: Some(value.clone()),
    }));
    Change {
        id: String::new(),
        object_id: object_id.into(),
        parent_ids: Vec::new(),
        ops,
        timestamp,
        author: "space-vanish-test".into(),
        snapshot: None,
        b: None,
    }
}

fn space(id: &str, timestamp: i64) -> Change {
    created(
        id,
        "channel",
        &[("name", json!({ "stringValue": id }))],
        timestamp,
    )
}

fn note(id: &str, name: &str, space: &str, timestamp: i64) -> Change {
    created(
        id,
        "note",
        &[
            ("name", json!({ "stringValue": name })),
            ("channel", json!({ "stringValue": space })),
        ],
        timestamp,
    )
}

/// Addressed and stored as a relay import stores it.
async fn record(engine: &Engine, mut change: Change) -> ChangeRecord {
    change.id = engine.change_id(&change).await.unwrap();
    let bytes = engine.encode_change(&change).await.unwrap();
    let decoded = engine.decode_change(&bytes).await.unwrap();
    ChangeRecord {
        id: decoded.id.clone(),
        object_id: decoded.object_id.clone(),
        json: serde_json::to_string(&decoded).unwrap(),
        bytes,
        b: None,
        rebased: false,
    }
}

async fn store_with(engine: &Engine, changes: Vec<Change>) -> Arc<SqliteChangeStore> {
    let store = Arc::new(SqliteChangeStore::in_memory().await.unwrap());
    let mut records = Vec::new();
    for change in changes {
        records.push(record(engine, change).await);
    }
    store.add_changes(records).await.unwrap();
    store
}

/// A replica over `store`, signed in as `keys`, offline, its keyring holding
/// a key per space (`owner`: an imported entry's owner), its session
/// installed without starting sync.
async fn replica(
    store: Arc<SqliteChangeStore>,
    keys: &Keys,
    keyring: &[(&str, Option<&Keys>)],
) -> ReplicaBackend {
    let engine = Engine::native().unwrap();
    engine.reset_all().await;
    let kv = Arc::new(InMemoryLocalKv::new());
    kv.set(RELAYS, Some("[]"));
    identity::save(&*kv, &keys.secret_hex()).unwrap();
    let channels: serde_json::Map<String, Json> = keyring
        .iter()
        .map(|(id, owner)| {
            let mut entry = json!({ "key": key_hex(), "keyId": 1, "createdAt": 1 });
            if let Some(owner) = owner {
                entry["owner"] = json!(owner.pubkey());
            }
            (id.to_string(), entry)
        })
        .collect();
    kv.set(
        SPACE_KEYS,
        Some(&json!({ "version": 1, "channels": channels }).to_string()),
    );
    let backend = ReplicaBackend::new(ReplicaParts {
        engine,
        store,
        kv,
        label: "unit replica".into(),
    });
    let session = Arc::new(backend.inner.new_session(keys.clone()));
    *backend.inner.session.lock() = Some(session);
    backend
}

/// Waits out background mutations and shared refreshes, then stops: a
/// straggler must not touch the engine's cache during the next test.
async fn settle(backend: &ReplicaBackend) {
    let start = Instant::now();
    loop {
        let busy = {
            let st = backend.inner.state.lock();
            st.mutations > 0 || st.shared_refreshes > 0
        };
        if !busy || start.elapsed() > WAIT {
            break;
        }
        roostr_nostr::sleep(Duration::from_millis(20)).await;
    }
    backend.stop();
}

/// The notes the object list shows (spaces list there too; `channels` covers them).
async fn notes(backend: &ReplicaBackend) -> Vec<String> {
    backend
        .objects()
        .await
        .unwrap()
        .into_iter()
        .filter(|o| o.type_key != "channel")
        .map(|o| o.name)
        .collect()
}

async fn channel_ids(backend: &ReplicaBackend) -> Vec<String> {
    backend
        .channels()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect()
}

async fn query_ids(backend: &ReplicaBackend, body: Json) -> Vec<String> {
    backend
        .query(&body)
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.id)
        .collect()
}

#[test]
fn a_vanished_space_takes_every_object_in_it_including_late_arrivals() {
    let _turn = TURN.lock();
    async_io::block_on(async {
        let engine = Engine::native().unwrap();
        let store = store_with(
            &engine,
            vec![
                space("gone", 100),
                note("before", "Before", "gone", 101),
                space("kept", 102),
                note("survivor", "Survivor", "kept", 103),
                created(
                    VANISH_LOG_ID,
                    "vanish_log",
                    &[("vanished:gone", json!({ "intValue": 100 }))],
                    104,
                ),
            ],
        )
        .await;
        let backend = replica(store.clone(), &member_keys(), &[]).await;
        assert_eq!(notes(&backend).await, ["Survivor"]);
        assert_eq!(channel_ids(&backend).await, ["kept"]);

        // A relay copy written into the space by a device that never heard
        // it went: it lands in a steady-state replay batch, not a full sweep.
        store
            .add_changes(vec![
                record(&engine, note("late", "Late", "gone", 105)).await,
            ])
            .await
            .unwrap();
        Host(Arc::downgrade(&backend.inner)).on_objects(vec!["late".into()]);
        assert_eq!(notes(&backend).await, ["Survivor"]);
        let queried = query_ids(&backend, json!({ "filters": [], "limit": 500 })).await;
        assert!(queried.contains(&"survivor".to_string()), "{queried:?}");
        assert!(!queried.contains(&"before".to_string()), "{queried:?}");
        assert!(!queried.contains(&"late".to_string()), "{queried:?}");
        // Held but gone: the digest covers kept, survivor and the ledger only.
        assert_eq!(backend.sync_digest().await.unwrap()["objects"], 3);
        settle(&backend).await;
    });
}

#[test]
fn spaces_report_their_owner_a_member_leaves_only_the_owner_deletes_for_everyone() {
    let _turn = TURN.lock();
    async_io::block_on(async {
        let (owner, member) = (owner_keys(), member_keys());
        let engine = Engine::native().unwrap();
        let store = store_with(
            &engine,
            vec![
                space("mine", 100),
                space("echo", 101),
                space("joined", 102),
                note("shared", "Shared note", "joined", 103),
                space("local", 104),
            ],
        )
        .await;
        let backend = replica(
            store,
            &member,
            &[
                ("mine", None),
                ("echo", Some(&member)),
                ("joined", Some(&owner)),
            ],
        )
        .await;
        // An imported entry naming this identity, or no keyring entry at
        // all, is this identity's own space.
        let owners: HashMap<String, String> = backend
            .channels()
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.owner))
            .collect();
        let expected: HashMap<String, String> = [
            ("mine", ""),
            ("echo", ""),
            ("joined", owner.pubkey()),
            ("local", ""),
        ]
        .into_iter()
        .map(|(id, o)| (id.to_string(), o.to_string()))
        .collect();
        assert_eq!(owners, expected);

        let refused = backend
            .mutate("vanish", obj(json!({ "object_ids": ["joined", "shared"] })))
            .await
            .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("Only the space's owner can delete it for everyone"),
            "{refused}"
        );
        for channel_id in ["mine", "echo", "local"] {
            let refused = backend
                .mutate("space_leave", obj(json!({ "channel_id": channel_id })))
                .await
                .unwrap_err();
            assert!(
                refused.to_string().contains("You own this space"),
                "{channel_id}: {refused}"
            );
        }
        assert_eq!(notes(&backend).await, ["Shared note"]);

        let left = backend
            .mutate("space_leave", obj(json!({ "channel_id": "joined" })))
            .await
            .unwrap();
        assert_eq!(left, obj(json!({ "ok": true, "left": "joined" })));
        let keyring = backend.keyring();
        assert!(keyring.get("joined").unwrap().is_none());
        assert!(keyring.get("mine").unwrap().is_some());
        assert_eq!(channel_ids(&backend).await, ["mine", "echo", "local"]);
        assert_eq!(notes(&backend).await, Vec::<String>::new());
        settle(&backend).await;
    });
}

#[test]
fn the_owners_signal_is_the_one_path_that_vanishes_a_space_someone_else_owns() {
    let _turn = TURN.lock();
    async_io::block_on(async {
        let engine = Engine::native().unwrap();
        let store = store_with(
            &engine,
            vec![
                space("joined", 100),
                note("shared", "Shared note", "joined", 101),
            ],
        )
        .await;
        let backend = replica(store, &member_keys(), &[("joined", Some(&owner_keys()))]).await;
        assert_eq!(channel_ids(&backend).await, ["joined"]);
        // What sync calls when the owner's h-deletion arrives.
        Host(Arc::downgrade(&backend.inner)).on_space_vanished("joined".into());
        let start = Instant::now();
        while !channel_ids(&backend).await.is_empty() {
            assert!(
                start.elapsed() < WAIT,
                "the owner's signal never vanished the space"
            );
            roostr_nostr::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(notes(&backend).await, Vec::<String>::new());
        // Unlike leaving, the key stays: it addresses this identity's own h-deletion.
        assert!(backend.keyring().get("joined").unwrap().is_some());
        settle(&backend).await;
    });
}

#[test]
fn cold_queries_and_engine_resets_cannot_resurrect_vanished_objects() {
    let _turn = TURN.lock();
    async_io::block_on(async {
        let engine = Engine::native().unwrap();
        let note_of = |i: i64, name: &str| {
            created(
                &format!("obj-{i}"),
                "note",
                &[
                    ("name", json!({ "stringValue": name })),
                    ("done", json!({ "boolValue": i % 2 == 0 })),
                ],
                i + 1,
            )
        };
        let store = store_with(
            &engine,
            vec![
                note_of(1, "Must stay vanished"),
                note_of(2, "Survivor"),
                created(
                    VANISH_LOG_ID,
                    "vanish_log",
                    &[("vanished:obj-1", json!({ "intValue": 100 }))],
                    100,
                ),
            ],
        )
        .await;
        // A warm database: every replay persisted, so no replay upserts mask
        // a wrongly loaded corpus.
        let warming = replica(store.clone(), &member_keys(), &[]).await;
        warming.inner.ensure().await.unwrap();
        settle(&warming).await;
        drop(warming);

        let backend = replica(store, &member_keys(), &[]).await;
        let body = json!({ "filters": [], "limit": 500 });
        let names = |result: QueryResult| -> Vec<String> {
            result.records.into_iter().filter_map(|r| r.name).collect()
        };
        assert_eq!(notes(&backend).await, ["Survivor"]);
        assert_eq!(names(backend.query(&body).await.unwrap()), ["Survivor"]);
        engine.reset_all().await;
        assert_eq!(names(backend.query(&body).await.unwrap()), ["Survivor"]);
        settle(&backend).await;
    });
}

#[test]
fn a_local_commit_is_applied_by_the_engine_and_never_upserted_again() {
    let _turn = TURN.lock();
    async_io::block_on(async {
        let engine = Engine::native().unwrap();
        let store = Arc::new(SqliteChangeStore::in_memory().await.unwrap());
        let backend = replica(store, &member_keys(), &[]).await;
        let inner = backend.inner.clone();
        let owed = || inner.state.lock().query_upserted.clone();
        let mutate = async |action: &str, params: Json| -> JsonMap {
            let out = backend.mutate(action, obj(params)).await.unwrap();
            // Nothing owed means nothing is pushed before the next plan.
            assert_eq!(owed(), HashSet::new(), "after {action}");
            out
        };
        let id = |out: JsonMap| out["id"].as_str().unwrap().to_string();

        let space = id(mutate("create", json!({ "type_key": "channel", "name": "Home" })).await);
        let note = id(mutate("create", json!({ "name": "Page" })).await);
        mutate(
            "block_add",
            json!({ "object_id": note, "block": { "content": { "text": { "text": "typed" } } }, "target_id": "", "position": 0 }),
        )
        .await;
        mutate(
            "set_field",
            json!({ "object_id": note, "key": "status", "value": { "stringValue": "draft" } }),
        )
        .await;
        // table_row_add plans from the table the cache holds: it must see the create.
        let table = id(mutate(
            "table_create",
            json!({ "object_id": note, "rows": 1, "cols": 2 }),
        )
        .await);
        mutate(
            "table_row_add",
            json!({ "object_id": note, "table_id": table }),
        )
        .await;
        mutate("delete", json!({ "object_id": note })).await;
        mutate("restore", json!({ "object_id": note })).await;

        // A relay echoing this device's own changes re-replays the object to
        // the same heads: still nothing owed.
        Host(Arc::downgrade(&inner)).on_objects(vec![note.clone(), space.clone()]);
        backend.objects().await.unwrap();
        assert_eq!(owed(), HashSet::new());

        // The cache the planner reads is the replica's replay, field for field.
        let rows = backend
            .query(&json!({ "filters": [{ "key": "id", "condition": "equal", "value": note }] }))
            .await
            .unwrap()
            .records;
        assert_eq!(rows.len(), 1);
        let replica_state = inner.state.lock().states[&note].clone();
        assert_eq!(rows[0].fields, backend.object(&note).await.unwrap().fields);
        let table_rows = replica_state["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["content"].get("tableRow").is_some())
            .count();
        assert_eq!(table_rows, 2);
        assert_eq!(owed(), HashSet::new());
        assert_eq!(heads(&inner.state.lock().states[&space]).len(), 1);

        // A cache whose state carries no heads (an older replay) refuses the
        // apply: the replayed state is owed as before, and once pushed with
        // its heads the next commit applies again.
        let mut headless = (*replica_state).clone();
        headless.as_object_mut().unwrap().remove("heads");
        {
            let _cache = inner.cache_lock.lock().await;
            engine
                .query(QueryUpdate {
                    upserts: &[headless],
                    removed: &[],
                    reset: false,
                    body: None,
                    now_ms: 0,
                })
                .await
                .unwrap();
        }
        backend
            .mutate(
                "set_field",
                obj(json!({ "object_id": note, "key": "status", "value": { "stringValue": "refused" } })),
            )
            .await
            .unwrap();
        assert_eq!(owed(), HashSet::from([note.clone()]));
        mutate(
            "set_field",
            json!({ "object_id": note, "key": "status", "value": { "stringValue": "applied" } }),
        )
        .await;
        let after = query_ids(
            &backend,
            json!({ "filters": [{ "key": "status", "condition": "equal", "value": "applied" }] }),
        )
        .await;
        assert_eq!(after, std::slice::from_ref(&note));
        assert_eq!(owed(), HashSet::new());
        settle(&backend).await;
    });
}
