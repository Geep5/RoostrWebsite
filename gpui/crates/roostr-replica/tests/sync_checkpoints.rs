//! Kind-1079 checkpoints: replay from a checkpoint, import through sync
//! (personal and shared, owner only), supersession, and an object held only
//! as a checkpoint showing after a cold and a warm boot. Replicas never build
//! checkpoints; a publisher's payload is hand-assembled from the wire layout
//! in core/proto.odin (Checkpoint: 1 objectId, 2 headIds, 3 coveredIds,
//! 4 Snapshot, 5 createdAt; Snapshot: 1 id, 2 typeKey, 3 {1 key, 2 Value},
//! 7 createdAt, 8 updatedAt; Value.stringValue = 1).
#![cfg(not(target_family = "wasm"))]

#[path = "support/space_fixture.rs"]
mod space_fixture;
mod support;

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use roostr_core::Backend;
use roostr_core::backend::JsonMap;
use roostr_engine::Change;
use roostr_nostr::{Event, EventTemplate, Keys, kinds};
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::{RELAYS, ReplicaBackend, ReplicaParts};
use roostr_replica_store::{ChangeStore, CheckpointRecord, SqliteChangeStore};
use serde_json::json;
use sha2::{Digest, Sha256};
use space_fixture::{SPACE_ID, World};
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, block_on, eventually};

const WAIT: Duration = Duration::from_secs(30);

fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while n >= 0x80 {
        out.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
    out
}

fn field(no: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint((no << 3) | 2);
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

fn checkpoint_bytes(
    object_id: &str,
    type_key: &str,
    fields: &[(&str, &str)],
    heads: &[&str],
    covered: &[&str],
    created_at: u64,
) -> Vec<u8> {
    let mut snapshot = field(1, object_id.as_bytes());
    snapshot.extend(field(2, type_key.as_bytes()));
    for (key, value) in fields {
        let mut entry = field(1, key.as_bytes());
        entry.extend(field(2, &field(1, value.as_bytes())));
        snapshot.extend(field(3, &entry));
    }
    for no in [7, 8] {
        snapshot.extend(varint(no << 3));
        snapshot.extend(varint(100));
    }
    let mut out = field(1, object_id.as_bytes());
    for id in heads {
        out.extend(field(2, &hex::decode(id).unwrap()));
    }
    for id in covered {
        out.extend(field(3, &hex::decode(id).unwrap()));
    }
    out.extend(field(4, &snapshot));
    out.extend(varint(5 << 3));
    out.extend(varint(created_at));
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `doc` created at "v1", renamed "v2"; c1 checkpoints the create, c2 both.
struct History {
    w: World,
    create_doc: Change,
    edit: Change,
    c1: Vec<u8>,
    c2: Vec<u8>,
}

fn name(value: &str) -> serde_json::Value {
    json!({"fieldSet": {"key": "name", "value": {"stringValue": value}}})
}

impl History {
    async fn new() -> History {
        let w = World::new(roostr_engine::Engine::native().unwrap()).await;
        let create_doc = w
            .change(
                "doc",
                json!([
                    {"objectCreate": {"typeKey": "note"}},
                    {"fieldSet": {"key": "channel", "value": {"stringValue": SPACE_ID}}},
                    name("v1"),
                ]),
                &[],
                None,
            )
            .await;
        let edit = w
            .change("doc", json!([name("v2")]), &[&create_doc.id], None)
            .await;
        let c1 = checkpoint_bytes(
            "doc",
            "note",
            &[("channel", SPACE_ID), ("name", "v1")],
            &[&create_doc.id],
            &[&create_doc.id],
            1,
        );
        let c2 = checkpoint_bytes(
            "doc",
            "note",
            &[("channel", SPACE_ID), ("name", "v2")],
            &[&edit.id],
            &[&create_doc.id, &edit.id],
            2,
        );
        History {
            w,
            create_doc,
            edit,
            c1,
            c2,
        }
    }

    /// Kind 1079: personal (sealed under the owner's conversation key, no
    /// tags) or shared (space key and stream, signed by `signer`).
    fn checkpoint_event(&self, bytes: &[u8], shared: bool, signer: &Keys) -> Event {
        let part = STANDARD.encode(bytes);
        if shared {
            return self
                .w
                .event(kinds::CHECKPOINT, &part, signer, &[], None, 100);
        }
        signer.sign(EventTemplate {
            kind: kinds::CHECKPOINT,
            created_at: 100,
            tags: Vec::new(),
            content: roostr_nostr::nip44::encrypt(&part, &self.w.personal_key()).unwrap(),
        })
    }
}

/// Runs one more history pass and waits for it to end.
async fn another_pass(replica: &Replica) {
    let before = replica.host.passes.lock().len();
    replica.sync.resume();
    let done = eventually(WAIT, async || replica.host.passes.lock().len() > before).await;
    assert!(done, "the catch-up pass ended");
}

#[test]
fn a_checkpoint_stands_in_for_the_history_it_covers() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let h = History::new().await;
        let engine = &h.w.engine;
        let both = [h.create_doc.clone(), h.edit.clone()];
        let full = engine.replay(&both, None).await.unwrap();
        assert!(full.is_some());
        assert_eq!(engine.replay(&[], Some(&h.c2)).await.unwrap(), full);
        assert_eq!(
            engine
                .replay(std::slice::from_ref(&h.edit), Some(&h.c1))
                .await
                .unwrap(),
            full
        );
        // A checkpoint that does not fit the held changes is ignored, never trusted over them.
        let stranger = checkpoint_bytes(
            "doc",
            "note",
            &[("name", "forged")],
            &[&h.edit.id],
            &[&"ab".repeat(32)],
            3,
        );
        assert_eq!(engine.replay(&both, Some(&stranger)).await.unwrap(), full);
    });
}

#[test]
fn personal_checkpoints_land_and_only_a_covering_one_supersedes() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let h = History::new().await;
        relay.insert(h.checkpoint_event(&h.c2, false, &h.w.owner));
        let replica = Replica::memory(h.w.owner.clone(), vec![relay.url.clone()]).await;
        replica
            .sync
            .set_shared_spaces(vec![h.w.space()])
            .await
            .unwrap();
        replica.sync.start().await.unwrap();
        replica.sync.await_first_walk().await;
        let held = replica
            .store
            .checkpoint("doc".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.heads, vec![h.edit.id.clone()]);
        assert_eq!(held.hash, sha256_hex(&h.c2));
        assert_eq!(replica.sync.stats().checkpoints, 1);

        // The narrower, older checkpoint arrives late: a clean no-op.
        relay.insert(h.checkpoint_event(&h.c1, false, &h.w.owner));
        another_pass(&replica).await;
        assert_eq!(
            replica.sync.stats().events,
            2,
            "the late checkpoint was received"
        );
        let still = replica
            .store
            .checkpoint("doc".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(still.hash, held.hash);
        assert_eq!(replica.sync.stats().checkpoints, 1);

        // Same covered set, different bytes: the higher hash wins on every replica.
        let rival_bytes = checkpoint_bytes(
            "doc",
            "note",
            &[("channel", SPACE_ID), ("name", "v2")],
            &[&h.edit.id],
            &[&h.create_doc.id, &h.edit.id],
            9,
        );
        let rival = CheckpointRecord {
            object_id: "doc".into(),
            hash: sha256_hex(&rival_bytes),
            bytes: rival_bytes,
            heads: held.heads.clone(),
            covered: 2,
        };
        let wins = rival.hash > held.hash;
        assert_eq!(
            replica
                .store
                .put_checkpoint(&h.w.engine, rival)
                .await
                .unwrap(),
            wins
        );
        replica.sync.stop();
    });
}

#[test]
fn an_incomparable_fork_of_equal_width_never_replaces_the_held_checkpoint() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let h = History::new().await;
        let store = SqliteChangeStore::in_memory().await.unwrap();
        assert!(
            store
                .put_checkpoint(
                    &h.w.engine,
                    CheckpointRecord {
                        object_id: "doc".into(),
                        hash: sha256_hex(&h.c2),
                        bytes: h.c2.clone(),
                        heads: vec![h.edit.id.clone()],
                        covered: 2,
                    }
                )
                .await
                .unwrap()
        );
        // Another device checkpointed a different branch of equal width:
        // neither covers the other, so the held one stays no matter the hash.
        let fork =
            h.w.change("doc", json!([name("fork")]), &[&h.create_doc.id], None)
                .await;
        let fork_bytes = checkpoint_bytes(
            "doc",
            "note",
            &[("channel", SPACE_ID), ("name", "fork")],
            &[&fork.id],
            &[&h.create_doc.id, &fork.id],
            2,
        );
        let fork_row = CheckpointRecord {
            object_id: "doc".into(),
            bytes: fork_bytes,
            hash: "ff".repeat(32),
            heads: vec![fork.id.clone()],
            covered: 2,
        };
        assert!(!store.put_checkpoint(&h.w.engine, fork_row).await.unwrap());
    });
}

#[test]
fn shared_checkpoints_are_accepted_from_the_space_owner_only() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let h = History::new().await;
        let w = &h.w;
        let replica = Replica::memory(w.owner.clone(), vec![relay.url.clone()]).await;
        let create_space = w.create_space().await;
        replica
            .store
            .add_changes(vec![w.record(&create_space).await])
            .await
            .unwrap();
        relay.insert(h.checkpoint_event(&h.c2, true, &w.member));
        replica
            .sync
            .set_shared_spaces(vec![w.space()])
            .await
            .unwrap();
        replica.sync.start().await.unwrap();
        replica.sync.await_first_walk().await;
        assert_eq!(replica.sync.stats().events, 1);
        assert_eq!(replica.store.checkpoint("doc".into()).await.unwrap(), None);

        relay.insert(h.checkpoint_event(&h.c2, true, &w.owner));
        // An owner's checkpoint cannot pull an object from another scope into this space.
        let foreign = checkpoint_bytes(
            "elsewhere",
            "note",
            &[("channel", "other"), ("name", "x")],
            &[&h.edit.id],
            &[&h.edit.id],
            1,
        );
        relay.insert(h.checkpoint_event(&foreign, true, &w.owner));
        another_pass(&replica).await;
        let held = replica.store.checkpoint("doc".into()).await.unwrap();
        assert_eq!(held.map(|c| c.heads), Some(vec![h.edit.id.clone()]));
        assert_eq!(
            replica.store.checkpoint("elsewhere".into()).await.unwrap(),
            None
        );
        replica.sync.stop();
    });
}

fn params(value: serde_json::Value) -> JsonMap {
    value.as_object().cloned().unwrap()
}

/// An offline replica backend over `dir` (an empty relay list is offline).
async fn open_backend(dir: &std::path::Path) -> ReplicaBackend {
    let store = Arc::new(
        SqliteChangeStore::open(dir.join("replica.sqlite"))
            .await
            .unwrap(),
    );
    let kv = Arc::new(FileLocalKv::open(dir.join("local.json")).unwrap());
    kv.set(RELAYS, Some("[]"));
    ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().unwrap(),
        store,
        kv,
        label: "checkpoint test".into(),
    })
}

#[test]
fn an_object_held_only_as_a_checkpoint_shows_after_a_cold_and_a_warm_boot() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let dir = tempfile::tempdir().unwrap();
    block_on(async {
        let h = History::new().await;
        let backend = open_backend(dir.path()).await;
        backend
            .mutate(
                "nostr_key_import",
                params(json!({ "key": h.w.owner.nsec() })),
            )
            .await
            .unwrap();
        backend.stop();
        drop(backend);

        // Sync brings the object in as nothing but a checkpoint.
        relay.insert(h.checkpoint_event(&h.c2, false, &h.w.owner));
        let replica = Replica::new(
            h.w.owner.clone(),
            vec![relay.url.clone()],
            SqliteChangeStore::open(dir.path().join("replica.sqlite"))
                .await
                .unwrap(),
        );
        replica.sync.start().await.unwrap();
        replica.sync.await_first_walk().await;
        replica.sync.stop();
        assert!(
            replica
                .store
                .changes_for("doc".into())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            replica
                .store
                .checkpoint("doc".into())
                .await
                .unwrap()
                .is_some()
        );
        replica.store.close().await.unwrap();
        drop(replica);

        // Cold boot, then a warm one over the replay memo the first wrote.
        for boot in ["cold", "warm"] {
            let backend = open_backend(dir.path()).await;
            let names: Vec<String> = backend
                .objects()
                .await
                .unwrap()
                .into_iter()
                .map(|o| o.name)
                .collect();
            assert_eq!(names, vec!["v2".to_string()], "{boot} boot objects");
            let found = backend
                .query(&json!({ "filters": [], "limit": 10 }))
                .await
                .unwrap();
            let names: Vec<Option<String>> = found.records.into_iter().map(|r| r.name).collect();
            assert_eq!(names, vec![Some("v2".to_string())], "{boot} boot query");
            backend.stop();
        }
    });
}
