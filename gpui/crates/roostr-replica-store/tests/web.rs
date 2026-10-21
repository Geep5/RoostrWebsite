//! IndexedDB backend in a real browser: write, read, close, reopen, destroy.
//!
//! Checkpoint writes take the engine (it judges supersession): `engine.wasm`
//! is fetched from `ROOSTR_ENGINE_TEST_BASE` (build time; default
//! `http://127.0.0.1:8787/`), as in roostr-engine's `tests/web.rs`.
#![cfg(target_family = "wasm")]

use roostr_replica_store::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

fn record(id: &str, object: &str, payload: u8) -> ChangeRecord {
    ChangeRecord {
        id: id.into(),
        object_id: object.into(),
        bytes: vec![1, payload, 0xFF],
        json: format!(r#"{{"id":"{id}"}}"#),
        b: Some("bb".into()),
        rebased: false,
    }
}

fn base(hash: &str, epoch: i64) -> BaseRecord {
    BaseRecord {
        object_id: "o1".into(),
        hash: hash.into(),
        epoch,
        created_at: 10 * epoch,
        prev_base: String::new(),
        bytes: vec![epoch as u8],
    }
}

async fn engine() -> roostr_engine::Engine {
    let base = option_env!("ROOSTR_ENGINE_TEST_BASE").unwrap_or("http://127.0.0.1:8787/");
    roostr_engine::Engine::load(&roostr_engine::WebSource::under(base))
        .await
        .expect("engine loads and verifies")
}

#[wasm_bindgen_test]
async fn write_read_reopen_round_trip() {
    const NAME: &str = "roostr-replica-smoke";
    IdbChangeStore::destroy(NAME).await.unwrap();
    let store = IdbChangeStore::open(NAME).await.unwrap();
    assert!(
        store.object_ids().await.unwrap().is_empty(),
        "fresh database (empty-index cursor guard)"
    );

    assert_eq!(
        store
            .add_changes(vec![
                record("z", "o1", 1),
                record("y", "o2", 2),
                record("z", "o1", 1)
            ])
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .add_changes(vec![record("a", "o1", 3), record("y", "o2", 2)])
            .await
            .unwrap(),
        1
    );
    let pending = PendingPublish {
        key: "s/1/c".into(),
        object_id: "o2".into(),
        change_id: "c".into(),
        bytes: vec![4],
        space_id: Some("s".into()),
        key_id: Some(1),
        base: None,
        events: None,
    };
    store
        .add_local_change(record("c", "o2", 4), pending.clone())
        .await
        .unwrap();
    store
        .add_rebased("a".into(), record("r", "o1", 5))
        .await
        .unwrap();
    store
        .set_cursor(77, Some(vec![("g".into(), 5)]))
        .await
        .unwrap();
    store.mark_published("z".into()).await.unwrap();
    assert!(
        store
            .put_checkpoint(
                &engine().await,
                CheckpointRecord {
                    object_id: "o1".into(),
                    bytes: vec![9],
                    hash: "cp".into(),
                    heads: vec!["h".into()],
                    covered: 2
                }
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .put_base(base("e1", 1), vec!["".into()])
            .await
            .unwrap()
    );
    assert!(
        !store
            .put_base(base("e1", 1), vec!["space".into()])
            .await
            .unwrap()
    );
    assert!(store.put_base(base("e10", 10), vec![]).await.unwrap());
    let held = |id: &str, created_at: i64| HeldEvent {
        id: id.into(),
        created_at,
        kind: 1078,
        b: Some("bb".into()),
    };
    store
        .record_held("".into(), vec![held("e2", 2), held("e1", 2), held("e0", 3)])
        .await
        .unwrap();
    let input = store.replay_inputs().await.unwrap()["o1"].clone();
    store
        .put_cached_replays(
            vec![CachedReplay {
                object_id: "o1".into(),
                input: input.clone(),
                state: b"{}".to_vec(),
            }],
            "v1".into(),
        )
        .await
        .unwrap();
    store.close().await.unwrap();
    assert_eq!(store.cursor().await, Err(StoreError::Closed));

    let store = IdbChangeStore::open(NAME).await.unwrap();
    let o1: Vec<_> = store.changes_for("o1".into()).await.unwrap();
    assert_eq!(
        o1.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["z", "a", "r"],
        "insertion order, not id order"
    );
    assert_eq!(o1[0], record("z", "o1", 1));
    assert!(o1[1].rebased);
    assert_eq!(store.object_ids().await.unwrap(), ["o1", "o2"]);
    assert_eq!(store.change_counts().await.unwrap()["o2"], 2);
    assert_eq!(store.cursor().await.unwrap(), 77);
    assert_eq!(store.replay_groups().await.unwrap(), [("g".into(), 5)]);
    assert_eq!(store.pending_publishes().await.unwrap(), [pending]);
    assert!(store.pending_change_ids().await.unwrap().contains("c"));
    assert!(store.is_published("z".into()).await.unwrap());
    assert_eq!(
        store.checkpoint("o1".into()).await.unwrap().unwrap().heads,
        ["h"]
    );
    let bases: Vec<_> = store
        .bases("o1".into())
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.hash)
        .collect();
    assert_eq!(bases, ["e10"], "epoch 1 pruned below the kept chain");
    assert_eq!(
        store.current_base("o1".into()).await.unwrap(),
        Some(CurrentBase {
            hash: "e10".into(),
            created_at: 100
        })
    );
    assert_eq!(store.objects_without_base().await.unwrap(), ["o2"]);
    let streams = store.newest_base_by_stream().await.unwrap();
    assert_eq!((streams[""], streams["space"]), (10, 10));
    let items = store
        .held_events(
            "".into(),
            HeldQuery {
                kinds: vec![1078],
                b: Some(vec!["bb".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["e1", "e2", "e0"]
    );
    let inputs = store.object_inputs("o1".into()).await.unwrap();
    assert_eq!((inputs.changes.len(), inputs.count), (2, 3));
    assert!(store.put_base(base("e11", 11), vec![]).await.unwrap());
    let cached = store
        .cached_replays(vec!["o1".into()], "v1".into())
        .await
        .unwrap();
    assert!(
        !cached["o1"].serves(&store.replay_inputs().await.unwrap()["o1"]),
        "the base added after the memo invalidates it"
    );
    assert!(cached["o1"].serves(&input));
    assert!(
        store
            .cached_replays(vec!["o1".into()], "v2".into())
            .await
            .unwrap()
            .is_empty()
    );

    store.clear().await.unwrap();
    assert!(store.object_ids().await.unwrap().is_empty());
    store.close().await.unwrap();
    IdbChangeStore::destroy(NAME).await.unwrap();
}

#[wasm_bindgen_test]
async fn an_import_is_one_transaction_and_the_publication_sweep_reads_marks_in_bulk() {
    const NAME: &str = "roostr-replica-import";
    IdbChangeStore::destroy(NAME).await.unwrap();
    let store = IdbChangeStore::open(NAME).await.unwrap();
    let held = |id: &str, created_at: i64| HeldEvent {
        id: id.into(),
        created_at,
        kind: 1078,
        b: None,
    };
    let query = HeldQuery {
        kinds: vec![1078],
        ..Default::default()
    };
    let page = ImportBatch {
        changes: vec![
            record("c1", "o1", 1),
            record("c2", "o1", 2),
            record("c3", "o2", 3),
            record("c3", "o2", 3),
        ],
        published: vec!["c1".into(), "s/1/c2".into(), "c3".into()],
        held: vec![
            ("".into(), held("e1", 10)),
            ("tag".into(), held("e2", 11)),
            ("".into(), held("e1", 10)),
        ],
    };
    store
        .add_changes(vec![record("c1", "o1", 1)])
        .await
        .unwrap();
    store
        .save_pending(PendingPublish {
            key: "s/1/c2".into(),
            object_id: "o1".into(),
            change_id: "c2".into(),
            bytes: vec![9],
            space_id: Some("s".into()),
            key_id: Some(1),
            base: None,
            events: None,
        })
        .await
        .unwrap();
    assert_eq!(store.import(page.clone()).await.unwrap(), 2, "c1 was held");
    let ids = |records: Vec<ChangeRecord>| records.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(
        ids(store.changes_for("o1".into()).await.unwrap()),
        ["c1", "c2"]
    );
    for key in ["c1", "s/1/c2", "c3"] {
        assert!(store.is_published(key.into()).await.unwrap(), "{key}");
    }
    assert_eq!(store.pending("s/1/c2".into()).await.unwrap(), None);
    assert_eq!(
        store
            .held_events("".into(), query.clone())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .held_events("tag".into(), query.clone())
            .await
            .unwrap()[0]
            .id,
        "e2"
    );
    // The same page again (a reload re-reading it) adds nothing.
    assert_eq!(store.import(page).await.unwrap(), 0);
    assert_eq!(store.import(ImportBatch::default()).await.unwrap(), 0);
    assert_eq!(store.held_events("".into(), query).await.unwrap().len(), 1);

    let owed =
        |prefix: &str| store.unpublished_changes(vec!["o1".into(), "o2".into()], prefix.into());
    assert_eq!(ids(owed("s/1/").await.unwrap()), ["c1", "c3"]);
    assert_eq!(ids(owed("").await.unwrap()), ["c2"]);
    assert_eq!(owed("s/1/").await.unwrap()[1], record("c3", "o2", 3));
    store.close().await.unwrap();
    IdbChangeStore::destroy(NAME).await.unwrap();
}
