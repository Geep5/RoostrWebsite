//! SQLite backend: the iOS `SQLiteChangeStoreTests` / `ReplayCacheTests`
//! store-level cases, plus the browser-only operations of `store.ts`.
#![cfg(not(target_family = "wasm"))]

use std::collections::HashSet;

use futures::executor::block_on;
use roostr_replica_store::*;

fn record(id: &str, object: &str, payload: u8) -> ChangeRecord {
    ChangeRecord {
        id: id.into(),
        object_id: object.into(),
        bytes: vec![0x01, payload, 0xFF],
        json: format!(
            r#"{{"id":"{id}","object_id":"{object}","n":{payload},"text":"quote\" back\\ nl\n tab\t ünï"}}"#
        ),
        b: None,
        rebased: false,
    }
}

fn base(object: &str, hash: &str, epoch: i64, created_at: i64) -> BaseRecord {
    BaseRecord {
        object_id: object.into(),
        hash: hash.into(),
        epoch,
        created_at,
        prev_base: String::new(),
        bytes: vec![epoch as u8, 7],
    }
}

/// A row whose bytes are no checkpoint: fine while nothing is held for the
/// object, since the engine only judges a candidate against a held one.
fn checkpoint(object: &str, hash: &str, covered: u64) -> CheckpointRecord {
    CheckpointRecord {
        object_id: object.into(),
        bytes: hash.as_bytes().to_vec(),
        hash: hash.into(),
        heads: vec!["h1".into(), "h2".into()],
        covered,
    }
}

fn native_engine() -> roostr_engine::Engine {
    roostr_engine::Engine::native().unwrap()
}

fn field(out: &mut Vec<u8>, no: u64, bytes: &[u8]) {
    for mut n in [(no << 3) | 2, bytes.len() as u64] {
        while n >= 0x80 {
            out.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
    }
    out.extend_from_slice(bytes);
}

/// A real Checkpoint protobuf (core/proto.odin: 1 objectId, 3 coveredIds,
/// 4 Snapshot{1 id, 2 typeKey}) covering one 32-byte id per byte of
/// `covered`; `salt` rides in the typeKey so equal sets differ in bytes.
fn covering(object: &str, covered: &[u8], salt: u32) -> CheckpointRecord {
    let mut bytes = Vec::new();
    field(&mut bytes, 1, object.as_bytes());
    for id in covered {
        field(&mut bytes, 3, &[*id; 32]);
    }
    let mut snapshot = Vec::new();
    field(&mut snapshot, 1, object.as_bytes());
    field(&mut snapshot, 2, format!("t{salt}").as_bytes());
    field(&mut bytes, 4, &snapshot);
    use sha2::Digest;
    let hash = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    CheckpointRecord {
        object_id: object.into(),
        bytes,
        hash,
        heads: Vec::new(),
        covered: covered.len() as u64,
    }
}

/// The first `covering(object, covered, salt)` from salt 1 whose hash is
/// above (`larger`) or below `than`.
fn covering_hashed(object: &str, covered: &[u8], than: &str, larger: bool) -> CheckpointRecord {
    (1..)
        .map(|salt| covering(object, covered, salt))
        .find(|record| record.hash != than && (record.hash.as_str() > than) == larger)
        .unwrap()
}

fn pending(key: &str, object: &str, change: &str) -> PendingPublish {
    PendingPublish {
        key: key.into(),
        object_id: object.into(),
        change_id: change.into(),
        bytes: vec![9, 8, 7],
        space_id: None,
        key_id: None,
        base: None,
        events: None,
    }
}

fn store() -> SqliteChangeStore {
    block_on(SqliteChangeStore::in_memory()).unwrap()
}

fn ids(records: &[ChangeRecord]) -> Vec<&str> {
    records.iter().map(|r| r.id.as_str()).collect()
}

#[test]
fn add_changes_dedupes_by_id_and_counts_new() {
    let store = store();
    let (a, b) = (record("a", "o1", 1), record("b", "o1", 2));
    block_on(async {
        assert_eq!(
            store.add_changes(vec![a.clone(), b.clone()]).await.unwrap(),
            2
        );
        assert_eq!(
            store
                .add_changes(vec![a.clone(), b.clone(), record("c", "o2", 3)])
                .await
                .unwrap(),
            1
        );
        assert_eq!(store.add_changes(vec![a.clone()]).await.unwrap(), 0);
        assert_eq!(store.add_changes(vec![]).await.unwrap(), 0);
        assert_eq!(
            store
                .add_changes(vec![record("d", "o2", 4), record("d", "o2", 4)])
                .await
                .unwrap(),
            1,
            "dedupe within a batch"
        );
        assert_eq!(
            store.changes_for("o1".into()).await.unwrap(),
            vec![a, b],
            "bytes and JSON text round-trip exactly"
        );
    });
}

#[test]
fn changes_for_preserves_insertion_order_per_object() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![record("z", "o1", 0), record("y", "o2", 10)])
            .await
            .unwrap();
        store.add_changes(vec![record("m", "o1", 1)]).await.unwrap();
        store
            .add_changes(vec![record("b", "o2", 11), record("a", "o1", 2)])
            .await
            .unwrap();
        assert_eq!(
            ids(&store.changes_for("o1".into()).await.unwrap()),
            ["z", "m", "a"]
        );
        assert_eq!(
            ids(&store.changes_for("o2".into()).await.unwrap()),
            ["y", "b"]
        );
        assert!(
            store
                .changes_for("missing".into())
                .await
                .unwrap()
                .is_empty()
        );
        let counts = store.change_counts().await.unwrap();
        assert_eq!((counts["o1"], counts["o2"]), (3, 2));
    });
}

#[test]
fn object_ids_cover_changes_checkpoints_and_bases() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![
                record("1", "o2", 0),
                record("2", "o1", 0),
                record("3", "o2", 0),
                record("4", "o1", 0),
            ])
            .await
            .unwrap();
        store
            .put_checkpoint(&native_engine(), checkpoint("cp-only", "aa", 1))
            .await
            .unwrap();
        store
            .put_base(base("base-only", "bb", 1, 5), vec![])
            .await
            .unwrap();
        store
            .put_checkpoint(&native_engine(), checkpoint("o1", "cc", 1))
            .await
            .unwrap();
        assert_eq!(
            store.object_ids().await.unwrap(),
            ["o2", "o1", "base-only", "cp-only"],
            "first-change order, then the rest"
        );
    });
}

#[test]
fn cursor_and_replay_groups_round_trip_together() {
    let store = store();
    block_on(async {
        assert_eq!(store.cursor().await.unwrap(), 0);
        assert!(store.replay_groups().await.unwrap().is_empty());
        store
            .set_cursor(
                1_700_000_000,
                Some(vec![("space/1".into(), 42), ("space/2".into(), 7)]),
            )
            .await
            .unwrap();
        assert_eq!(store.cursor().await.unwrap(), 1_700_000_000);
        assert_eq!(
            store.replay_groups().await.unwrap(),
            [("space/1".into(), 42), ("space/2".into(), 7)]
        );
        store.set_cursor(1_700_000_001, None).await.unwrap();
        assert_eq!(
            store.replay_groups().await.unwrap().len(),
            2,
            "no groups given: the stored ones stay"
        );
        store.set_cursor(1_700_000_002, Some(vec![])).await.unwrap();
        assert_eq!(store.cursor().await.unwrap(), 1_700_000_002);
        assert!(store.replay_groups().await.unwrap().is_empty());
    });
}

#[test]
fn replay_groups_written_by_the_browser_decode() {
    let store = store();
    block_on(async {
        store
            .set_meta("replay-groups".into(), Some(r#"[["g",1.7e12]]"#.into()))
            .await
            .unwrap();
        assert_eq!(
            store.replay_groups().await.unwrap(),
            [("g".into(), 1_700_000_000_000)]
        );
        store
            .set_meta("replay-groups".into(), Some(r#"[["g"]]"#.into()))
            .await
            .unwrap();
        assert!(matches!(
            store.replay_groups().await,
            Err(StoreError::Corrupt(_))
        ));
    });
}

#[test]
fn bootstrap_lifecycle() {
    let store = store();
    block_on(async {
        assert!(!store.bootstrapped().await.unwrap());
        assert_eq!(store.bootstrap_floor().await.unwrap(), None);
        store.set_bootstrap_floor(Some(1234)).await.unwrap();
        assert_eq!(store.bootstrap_floor().await.unwrap(), Some(1234));
        store.set_bootstrap_floor(Some(99)).await.unwrap();
        assert_eq!(store.bootstrap_floor().await.unwrap(), Some(99));
        store.set_bootstrapped().await.unwrap();
        store.set_bootstrapped().await.unwrap();
        assert!(store.bootstrapped().await.unwrap());
        store.set_bootstrap_floor(None).await.unwrap();
        assert_eq!(store.bootstrap_floor().await.unwrap(), None);
        assert!(store.bootstrapped().await.unwrap());
    });
}

#[test]
fn meta_rows_upsert_and_delete() {
    let store = store();
    block_on(async {
        assert_eq!(store.meta("relays".into()).await.unwrap(), None);
        store
            .set_meta("relays".into(), Some("[\"wss://a\"]".into()))
            .await
            .unwrap();
        store
            .set_meta("relays".into(), Some("[\"wss://b\"]".into()))
            .await
            .unwrap();
        assert_eq!(
            store.meta("relays".into()).await.unwrap().as_deref(),
            Some("[\"wss://b\"]")
        );
        store.set_meta("relays".into(), None).await.unwrap();
        assert_eq!(store.meta("relays".into()).await.unwrap(), None);
    });
}

#[test]
fn pending_and_published_lifecycle() {
    let store = store();
    let event = serde_json::json!({"id": "e1", "pubkey": "p", "created_at": 5, "kind": 1078, "tags": [["h", "s1"]], "content": "c", "sig": "s"});
    let personal = pending("c1", "o1", "c1");
    let shared = PendingPublish {
        space_id: Some("s1".into()),
        key_id: Some(3),
        events: Some(vec![event.clone()]),
        ..pending("s1/3/c2", "o2", "c2")
    };
    block_on(async {
        assert_eq!(store.pending("c1".into()).await.unwrap(), None);
        assert!(!store.is_published("c1".into()).await.unwrap());
        assert!(store.pending_publishes().await.unwrap().is_empty());

        store.save_pending(personal.clone()).await.unwrap();
        store.save_pending(shared.clone()).await.unwrap();
        assert_eq!(
            store.pending("c1".into()).await.unwrap().as_ref(),
            Some(&personal)
        );
        assert_eq!(
            store.pending("s1/3/c2".into()).await.unwrap().as_ref(),
            Some(&shared)
        );
        let keys: Vec<_> = store
            .pending_publishes()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.key)
            .collect();
        assert_eq!(keys, ["c1", "s1/3/c2"]);
        assert_eq!(
            store.pending_change_ids().await.unwrap(),
            HashSet::from(["c1".to_owned(), "c2".to_owned()])
        );

        let retried = PendingPublish {
            events: Some(vec![event]),
            ..personal.clone()
        };
        store.save_pending(retried.clone()).await.unwrap();
        assert_eq!(store.pending("c1".into()).await.unwrap(), Some(retried));
        assert_eq!(store.pending_publishes().await.unwrap().len(), 2);

        store.mark_published("c1".into()).await.unwrap();
        assert!(store.is_published("c1".into()).await.unwrap());
        assert_eq!(store.pending("c1".into()).await.unwrap(), None);
        let keys: Vec<_> = store
            .pending_publishes()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.key)
            .collect();
        assert_eq!(keys, ["s1/3/c2"]);
        assert!(!store.is_published("s1/3/c2".into()).await.unwrap());

        store.mark_published("never-pending".into()).await.unwrap();
        assert!(store.is_published("never-pending".into()).await.unwrap());
        assert_eq!(store.pending_publishes().await.unwrap().len(), 1);

        // Meta rows that merely share the prefix's first letters are not obligations.
        store
            .set_meta("pending".into(), Some("x".into()))
            .await
            .unwrap();
        store
            .set_meta("pendingz".into(), Some("x".into()))
            .await
            .unwrap();
        assert_eq!(store.pending_publishes().await.unwrap().len(), 1);
    });
}

#[test]
fn local_change_stores_change_and_obligation_together() {
    let store = store();
    let change = record("c9", "o1", 1);
    let obligation = PendingPublish {
        base: Some("bb".into()),
        ..pending("c9", "o1", "c9")
    };
    block_on(async {
        store
            .add_local_change(change.clone(), obligation.clone())
            .await
            .unwrap();
        assert_eq!(store.changes_for("o1".into()).await.unwrap(), vec![change]);
        assert_eq!(store.pending("c9".into()).await.unwrap(), Some(obligation));
        assert!(store.pending_change_ids().await.unwrap().contains("c9"));
    });
}

#[test]
fn held_events_are_scoped_deduped_filtered_and_sorted() {
    let store = store();
    let held = |id: &str, created_at: i64, kind: u32, b: Option<&str>| HeldEvent {
        id: id.into(),
        created_at,
        kind,
        b: b.map(Into::into),
    };
    let query = |kinds: &[u32]| HeldQuery {
        kinds: kinds.to_vec(),
        ..Default::default()
    };
    block_on(async {
        store
            .record_held(
                "".into(),
                vec![
                    held("3", 300, 1078, None),
                    held("1", 100, 1078, Some("b1")),
                    held("2", 100, 1079, None),
                    held("0", 300, 1078, None),
                ],
            )
            .await
            .unwrap();
        store
            .record_held(
                "space".into(),
                vec![
                    held("3", 300, 1078, None),
                    held("4", 400, 1078, Some("b1")),
                    held("5", 500, 5, None),
                ],
            )
            .await
            .unwrap();
        // First record per (scope, id) wins.
        store
            .record_held("".into(), vec![held("1", 999, 1, None)])
            .await
            .unwrap();

        let items = store
            .held_events("".into(), query(&[1078, 1079, 1]))
            .await
            .unwrap();
        let order: Vec<_> = items
            .iter()
            .map(|i| (i.created_at, i.id.as_str()))
            .collect();
        assert_eq!(
            order,
            [(100, "1"), (100, "2"), (300, "0"), (300, "3")],
            "sorted by (created_at, id); first record wins"
        );

        let space: Vec<_> = store
            .held_events("space".into(), query(&[1078, 5]))
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(
            space,
            ["3", "4", "5"],
            "an event held by two streams is in both"
        );
        let bounded = store
            .held_events(
                "".into(),
                HeldQuery {
                    kinds: vec![1078],
                    since: Some(200),
                    until: Some(300),
                    b: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            bounded.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["0", "3"]
        );
        let by_base = store
            .held_events(
                "space".into(),
                HeldQuery {
                    kinds: vec![1078],
                    b: Some(vec!["b1".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            by_base,
            [HeldItem {
                created_at: 400,
                id: "4".into()
            }],
            "#b keeps only deltas on those bases, in scope"
        );
        assert!(
            store
                .held_events("".into(), query(&[]))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .held_events("other".into(), query(&[1078]))
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn checkpoint_supersession_is_containment_then_hash() {
    let store = store();
    let engine = native_engine();
    block_on(async {
        assert_eq!(store.checkpoint("o1".into()).await.unwrap(), None);
        let held = covering("o1", &[1, 2], 0);
        assert!(store.put_checkpoint(&engine, held.clone()).await.unwrap());
        assert!(
            !store.put_checkpoint(&engine, held.clone()).await.unwrap(),
            "same checkpoint"
        );
        assert!(
            !store
                .put_checkpoint(&engine, covering_hashed("o1", &[1, 2], &held.hash, false))
                .await
                .unwrap(),
            "smaller hash at equal coverage"
        );
        assert!(
            !store
                .put_checkpoint(&engine, covering_hashed("o1", &[1], &held.hash, true))
                .await
                .unwrap(),
            "covers less"
        );
        assert!(
            !store
                .put_checkpoint(&engine, covering_hashed("o1", &[1, 3], &held.hash, true))
                .await
                .unwrap(),
            "an incomparable fork of equal width, larger hash"
        );
        assert!(
            !store
                .put_checkpoint(&engine, covering_hashed("o1", &[1, 3, 4], &held.hash, true))
                .await
                .unwrap(),
            "an incomparable fork covering more"
        );
        let twin = covering_hashed("o1", &[1, 2], &held.hash, true);
        assert!(
            store.put_checkpoint(&engine, twin.clone()).await.unwrap(),
            "larger hash at equal coverage"
        );
        let wider = covering_hashed("o1", &[1, 2, 3], &twin.hash, false);
        assert!(
            store.put_checkpoint(&engine, wider.clone()).await.unwrap(),
            "covers more, whatever the hash"
        );
        assert_eq!(store.checkpoint("o1".into()).await.unwrap(), Some(wider));
        store
            .put_checkpoint(&engine, checkpoint("o2", "ab", 1))
            .await
            .unwrap();
        let all = store.all_checkpoints().await.unwrap();
        assert_eq!(
            (all.len(), all["o2"].heads.as_slice()),
            (2, ["h1".to_owned(), "h2".to_owned()].as_slice())
        );
    });
}

#[test]
fn forgetting_checkpoint_floors_drops_bootstrapped_only_when_a_floor_shortened_a_walk() {
    let (shortened, zero, none) = (store(), store(), store());
    block_on(async {
        shortened.set_bootstrapped().await.unwrap();
        shortened
            .set_meta(
                "checkpoint-floors".into(),
                Some(r#"{"o1":0,"o2":17}"#.into()),
            )
            .await
            .unwrap();
        shortened.forget_checkpoint_floors().await.unwrap();
        assert_eq!(
            shortened.meta("checkpoint-floors".into()).await.unwrap(),
            None
        );
        assert!(!shortened.bootstrapped().await.unwrap());

        zero.set_bootstrapped().await.unwrap();
        zero.set_meta("checkpoint-floors".into(), Some(r#"{"o1":0}"#.into()))
            .await
            .unwrap();
        zero.forget_checkpoint_floors().await.unwrap();
        assert_eq!(zero.meta("checkpoint-floors".into()).await.unwrap(), None);
        assert!(zero.bootstrapped().await.unwrap());

        none.set_bootstrapped().await.unwrap();
        none.forget_checkpoint_floors().await.unwrap();
        assert!(none.bootstrapped().await.unwrap());
    });
}

#[test]
fn bases_track_current_streams_and_prune_the_chain() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![record("c1", "o1", 0), record("c2", "o2", 0)])
            .await
            .unwrap();
        assert_eq!(store.objects_without_base().await.unwrap(), ["o1", "o2"]);

        assert!(
            store
                .put_base(base("o1", "aa", 1, 100), vec!["".into()])
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_base(base("o1", "aa", 1, 100), vec!["space".into()])
                .await
                .unwrap(),
            "held by hash"
        );
        let streams = store.newest_base_by_stream().await.unwrap();
        assert_eq!(
            (streams[""], streams["space"]),
            (100, 100),
            "a held base still moves a new stream's floor"
        );

        // Same epoch: later created_at wins; then larger hash.
        assert!(
            store
                .put_base(base("o1", "bb", 1, 200), vec!["".into()])
                .await
                .unwrap()
        );
        assert!(
            store
                .put_base(base("o1", "ab", 1, 200), vec![])
                .await
                .unwrap()
        );
        assert_eq!(
            store.current_base("o1".into()).await.unwrap(),
            Some(CurrentBase {
                hash: "bb".into(),
                created_at: 200
            })
        );
        assert!(
            store
                .put_base(base("o1", "00", 2, 50), vec!["".into()])
                .await
                .unwrap()
        );
        assert_eq!(
            store.current_base("o1".into()).await.unwrap().unwrap().hash,
            "00",
            "higher epoch beats a later timestamp"
        );
        assert_eq!(
            store.newest_base_by_stream().await.unwrap()[""],
            200,
            "floors never move back"
        );

        store
            .put_base(base("o2", "ff", 3, 1), vec![])
            .await
            .unwrap();
        assert_eq!(store.current_base_hashes().await.unwrap(), ["00", "ff"]);
        let current = store.current_bases().await.unwrap();
        assert_eq!(
            (current["o1"].hash.as_str(), current["o2"].hash.as_str()),
            ("00", "ff")
        );
        assert!(store.objects_without_base().await.unwrap().is_empty());
        let held: Vec<_> = store
            .bases("o1".into())
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.hash)
            .collect();
        assert_eq!(held, ["aa", "ab", "bb", "00"], "current-base order");

        // Epoch 10 prunes epochs <= 10 - BASE_CHAIN_KEPT = 2.
        assert!(
            store
                .put_base(base("o1", "e10", 10, 1), vec![])
                .await
                .unwrap()
        );
        let held: Vec<_> = store
            .bases("o1".into())
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.epoch)
            .collect();
        assert_eq!(held, [10]);
        assert!(
            store
                .put_base(base("o1", "e3", 3, 1), vec![])
                .await
                .unwrap()
        );
        assert_eq!(
            store.bases("o1".into()).await.unwrap().len(),
            2,
            "an older epoch inside the window is kept"
        );
        assert_eq!(
            store.current_base("o1".into()).await.unwrap().unwrap().hash,
            "e10"
        );
    });
}

#[test]
fn rebase_marks_the_orphan_and_object_inputs_skip_it() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![
                record("a", "o1", 0),
                record("orphan", "o1", 1),
                record("x", "o1", 2),
            ])
            .await
            .unwrap();
        store
            .add_rebased("orphan".into(), record("rebased", "o1", 3))
            .await
            .unwrap();
        store
            .mark_rebased(vec!["x".into(), "missing".into()])
            .await
            .unwrap();
        let all = store.changes_for("o1".into()).await.unwrap();
        assert_eq!(ids(&all), ["a", "orphan", "x", "rebased"]);
        assert_eq!(
            all.iter()
                .filter(|c| c.rebased)
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["orphan", "x"]
        );

        store
            .put_checkpoint(&native_engine(), checkpoint("o1", "cp", 2))
            .await
            .unwrap();
        store
            .put_base(base("o1", "bb", 1, 9), vec![])
            .await
            .unwrap();
        let inputs = store.object_inputs("o1".into()).await.unwrap();
        assert_eq!(ids(&inputs.changes), ["a", "rebased"]);
        assert_eq!(inputs.count, 4);
        assert_eq!(inputs.checkpoint.unwrap().hash, "cp");
        assert_eq!(inputs.bases.len(), 1);
        assert_eq!(
            inputs.current_base,
            Some(CurrentBase {
                hash: "bb".into(),
                created_at: 9
            })
        );
        assert_eq!(
            store.object_inputs("none".into()).await.unwrap(),
            ObjectInputs::default()
        );
    });
}

#[test]
fn change_histories_group_bytes_by_object() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![
                record("a", "o1", 1),
                record("b", "o2", 2),
                record("c", "o1", 3),
            ])
            .await
            .unwrap();
        store
            .put_checkpoint(&native_engine(), checkpoint("o3", "cp", 1))
            .await
            .unwrap();
        store
            .put_checkpoint(&native_engine(), checkpoint("o1", "c1", 1))
            .await
            .unwrap();
        let histories = store.all_change_histories().await.unwrap();
        let summary: Vec<_> = histories
            .iter()
            .map(|h| {
                (
                    h.object_id.as_str(),
                    h.changes.len(),
                    h.checkpoint.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [("o1", 2, true), ("o2", 1, false), ("o3", 0, true)]
        );
        assert_eq!(histories[0].changes, [vec![1, 1, 0xFF], vec![1, 3, 0xFF]]);
    });
}

#[test]
fn replay_inputs_match_what_replay_consumes_and_move_on_every_input_change() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![
                record("a", "o1", 0),
                record("b", "o1", 1),
                record("c", "o2", 0),
            ])
            .await
            .unwrap();
        let inputs = store.replay_inputs().await.unwrap();
        assert_eq!(inputs["o1"], ReplayInput::new(["a", "b"], "", []));
        assert_eq!(
            inputs["o1"].change_digest,
            "911169ddaaf146aff539f58c26c489af3b892dff0fe283c1c264c65ae5aa59a2"
        );
        assert_eq!(inputs["o2"], ReplayInput::new(["c"], "", []));

        let engine = "core/1".to_owned();
        let memo = CachedReplay {
            object_id: "o1".into(),
            input: inputs["o1"].clone(),
            state: b"{\"v\":1}".to_vec(),
        };
        store
            .put_cached_replays(vec![memo.clone()], engine.clone())
            .await
            .unwrap();
        let rows = store
            .cached_replays(vec!["o1".into(), "o2".into()], engine.clone())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows["o1"].serves(&inputs["o1"]),
            "a row whose input matches is served"
        );
        assert!(
            store
                .cached_replays(vec!["o1".into()], "an-older-core/1".into())
                .await
                .unwrap()
                .is_empty(),
            "another engine never served"
        );

        let mut previous = inputs["o1"].clone();
        let mut assert_moved = |now: ReplayInput, why: &str| {
            assert!(!memo.serves(&now), "{why} invalidates the memo");
            assert_ne!(now, previous, "{why}");
            previous = now;
        };
        store.add_changes(vec![record("d", "o1", 2)]).await.unwrap();
        assert_moved(
            store.replay_inputs().await.unwrap().remove("o1").unwrap(),
            "a new change",
        );
        store
            .put_checkpoint(&native_engine(), checkpoint("o1", "cp", 3))
            .await
            .unwrap();
        assert_moved(
            store.replay_inputs().await.unwrap().remove("o1").unwrap(),
            "a new checkpoint",
        );
        store
            .put_base(base("o1", "bb", 1, 1), vec![])
            .await
            .unwrap();
        assert_moved(
            store.replay_inputs().await.unwrap().remove("o1").unwrap(),
            "a new base",
        );
        store.mark_rebased(vec!["d".into()]).await.unwrap();
        let now = store.replay_inputs().await.unwrap().remove("o1").unwrap();
        assert_eq!(
            now,
            ReplayInput::new(["a", "b"], "cp", ["bb"]),
            "rebased orphans leave the input"
        );
        assert_moved(now, "a rebase");

        store
            .put_checkpoint(&native_engine(), checkpoint("cp-only", "zz", 1))
            .await
            .unwrap();
        assert_eq!(
            store.replay_inputs().await.unwrap()["cp-only"],
            ReplayInput::new([], "zz", [])
        );

        store.clear_cached_replays().await.unwrap();
        assert!(
            store
                .cached_replays(vec!["o1".into()], engine)
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn clear_empties_every_table() {
    let store = store();
    block_on(async {
        store
            .add_local_change(record("a", "o1", 0), pending("a", "o1", "a"))
            .await
            .unwrap();
        store
            .put_checkpoint(&native_engine(), checkpoint("o1", "cp", 1))
            .await
            .unwrap();
        store
            .put_base(base("o1", "bb", 1, 1), vec!["".into()])
            .await
            .unwrap();
        store
            .record_held(
                "".into(),
                vec![HeldEvent {
                    id: "e".into(),
                    created_at: 1,
                    kind: 1078,
                    b: None,
                }],
            )
            .await
            .unwrap();
        store.set_cursor(5, None).await.unwrap();
        let input = store.replay_inputs().await.unwrap()["o1"].clone();
        store
            .put_cached_replays(
                vec![CachedReplay {
                    object_id: "o1".into(),
                    input,
                    state: vec![1],
                }],
                "v".into(),
            )
            .await
            .unwrap();
        store.clear().await.unwrap();
        assert!(store.object_ids().await.unwrap().is_empty());
        assert!(store.pending_publishes().await.unwrap().is_empty());
        assert_eq!(store.cursor().await.unwrap(), 0);
        assert!(store.newest_base_by_stream().await.unwrap().is_empty());
        assert!(
            store
                .held_events(
                    "".into(),
                    HeldQuery {
                        kinds: vec![1078],
                        ..Default::default()
                    }
                )
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .cached_replays(vec!["o1".into()], "v".into())
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn opening_an_older_database_migrates_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changes.sqlite");
    {
        // The iOS layout before `b`, `rebased`, `base_hashes` and scoped `held`.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('cursor', '42');
             CREATE TABLE changes (id TEXT PRIMARY KEY, object_id TEXT NOT NULL, seq INTEGER NOT NULL, bytes BLOB NOT NULL, json TEXT NOT NULL);
             INSERT INTO changes VALUES ('old', 'o1', 1, x'01', '{}');
             CREATE TABLE held (id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, kind INTEGER NOT NULL, author TEXT NOT NULL, h TEXT);
             INSERT INTO held VALUES ('x', 1, 1078, 'a', NULL);
             CREATE TABLE replay_cache (object_id TEXT PRIMARY KEY, engine_version TEXT NOT NULL, change_count INTEGER NOT NULL,
                 change_digest TEXT NOT NULL, checkpoint_hash TEXT NOT NULL, state BLOB NOT NULL);",
        )
        .unwrap();
    }
    block_on(async {
        let store = SqliteChangeStore::open(&path).await.unwrap();
        assert_eq!(store.cursor().await.unwrap(), 42);
        let old = store.changes_for("o1".into()).await.unwrap();
        assert_eq!(
            (old[0].id.as_str(), old[0].b.as_deref(), old[0].rebased),
            ("old", None, false)
        );
        assert_eq!(
            store
                .add_changes(vec![record("new", "o1", 0)])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            ids(&store.changes_for("o1".into()).await.unwrap()),
            ["old", "new"]
        );
        let query = HeldQuery {
            kinds: vec![1078],
            ..Default::default()
        };
        assert!(
            store
                .held_events("".into(), query.clone())
                .await
                .unwrap()
                .is_empty(),
            "unscoped held rows are re-fetched once"
        );
        store
            .record_held(
                "".into(),
                vec![HeldEvent {
                    id: "x".into(),
                    created_at: 1,
                    kind: 1078,
                    b: None,
                }],
            )
            .await
            .unwrap();
        assert_eq!(store.held_events("".into(), query).await.unwrap().len(), 1);
        store.close().await.unwrap();
    });
    let version: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn file_backed_store_survives_reopen_and_destroy_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changes.sqlite");
    let (a, b) = (record("a", "o1", 1), record("b", "o1", 2));
    block_on(async {
        let first = SqliteChangeStore::open(&path).await.unwrap();
        first.add_changes(vec![a.clone(), b.clone()]).await.unwrap();
        first
            .set_cursor(77, Some(vec![("g".into(), 1)]))
            .await
            .unwrap();
        first.set_bootstrapped().await.unwrap();
        first.save_pending(pending("b", "o1", "b")).await.unwrap();
        first.mark_published("a".into()).await.unwrap();
        first.close().await.unwrap();

        let second = SqliteChangeStore::open(&path).await.unwrap();
        assert_eq!(
            second.changes_for("o1".into()).await.unwrap(),
            vec![a.clone(), b]
        );
        assert_eq!(second.add_changes(vec![a]).await.unwrap(), 0);
        assert_eq!(second.cursor().await.unwrap(), 77);
        assert_eq!(second.replay_groups().await.unwrap(), [("g".into(), 1)]);
        assert!(second.bootstrapped().await.unwrap());
        assert_eq!(second.pending_publishes().await.unwrap().len(), 1);
        assert!(second.is_published("a".into()).await.unwrap());
        second.close().await.unwrap();

        SqliteChangeStore::destroy(&path).await.unwrap();
        assert!(!path.exists());
        let fresh = SqliteChangeStore::open(&path).await.unwrap();
        assert!(fresh.object_ids().await.unwrap().is_empty());
    });
}

#[test]
fn closed_store_fails_with_closed() {
    let store = store();
    block_on(async {
        store.close().await.unwrap();
        store.close().await.unwrap();
        assert_eq!(store.cursor().await, Err(StoreError::Closed));
        assert_eq!(
            store.add_changes(vec![record("a", "o", 0)]).await,
            Err(StoreError::Closed)
        );
    });
}

#[test]
fn store_is_usable_as_a_shared_trait_object() {
    let store: std::sync::Arc<dyn ChangeStore> = std::sync::Arc::new(store());
    let worker = {
        let store = store.clone();
        std::thread::spawn(move || block_on(store.add_changes(vec![record("t", "o", 0)])).unwrap())
    };
    assert_eq!(worker.join().unwrap(), 1);
    assert_eq!(block_on(store.change_counts()).unwrap()["o"], 1);
}

#[test]
fn an_import_writes_a_pages_changes_marks_and_held_events_in_one_go() {
    let store = store();
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
    block_on(async {
        store
            .add_changes(vec![record("c1", "o1", 1)])
            .await
            .unwrap();
        store
            .save_pending(pending("s/1/c2", "o1", "c2"))
            .await
            .unwrap();
        assert_eq!(store.import(page.clone()).await.unwrap(), 2, "c1 was held");
        assert_eq!(
            ids(&store.changes_for("o1".into()).await.unwrap()),
            ["c1", "c2"]
        );
        assert_eq!(ids(&store.changes_for("o2".into()).await.unwrap()), ["c3"]);
        for key in ["c1", "s/1/c2", "c3"] {
            assert!(store.is_published(key.into()).await.unwrap(), "{key}");
        }
        assert_eq!(store.pending("s/1/c2".into()).await.unwrap(), None);
        let scope = |scope: &str| store.held_events(scope.into(), query.clone());
        assert_eq!(scope("").await.unwrap().len(), 1);
        assert_eq!(scope("tag").await.unwrap()[0].id, "e2");

        // The same page again (a reload re-reading it) adds nothing.
        assert_eq!(store.import(page).await.unwrap(), 0);
        assert_eq!(store.import(ImportBatch::default()).await.unwrap(), 0);
        assert_eq!(store.changes_for("o1".into()).await.unwrap().len(), 2);
        assert_eq!(scope("").await.unwrap().len(), 1);
    });
}

#[test]
fn unpublished_changes_are_those_without_a_mark_under_the_prefix() {
    let store = store();
    block_on(async {
        store
            .add_changes(vec![
                record("c1", "o1", 1),
                record("c2", "o1", 2),
                record("c3", "o2", 3),
                record("c4", "o3", 4),
            ])
            .await
            .unwrap();
        store.mark_published("s/1/c2".into()).await.unwrap();
        store.mark_published("c1".into()).await.unwrap();
        let owed =
            |prefix: &str| store.unpublished_changes(vec!["o1".into(), "o2".into()], prefix.into());
        assert_eq!(ids(&owed("s/1/").await.unwrap()), ["c1", "c3"]);
        assert_eq!(ids(&owed("").await.unwrap()), ["c2", "c3"]);
        let all = owed("s/1/").await.unwrap();
        assert_eq!(all[1], record("c3", "o2", 3), "the whole row");
        assert!(
            store
                .unpublished_changes(Vec::new(), "".into())
                .await
                .unwrap()
                .is_empty()
        );
    });
}
