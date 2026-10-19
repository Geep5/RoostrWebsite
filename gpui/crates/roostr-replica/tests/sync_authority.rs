//! Shared-space authority and chunk provenance: the engine's authority gate
//! as the replica calls it (signer and held membership, never the inner
//! author), a membership removal taking effect inside one import batch, and
//! the receive session's chunk groups (provenance, assembly scope, malformed
//! and expired parts, the group cap). Native only; the engine's receive
//! session is one per process, so tests take turns (`ENGINE_TURN`).
#![cfg(not(target_family = "wasm"))]

#[path = "support/space_fixture.rs"]
mod space_fixture;
mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use roostr_engine::{
    AuthorizeSubject, Change, EngineError, SessionOpen, SessionSpace, SharedProvenance,
    SpaceAuthority,
};
use roostr_nostr::kinds;
use roostr_replica_store::{ChangeStore, SqliteChangeStore};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use space_fixture::{SPACE_ID, World, keys, relay_event};
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, block_on};

/// A receive-session clock: the session expires partial groups after five minutes.
const NOW_MS: i64 = 1_700_000_000_000;
const GROUP_TTL_MS: i64 = 300_000;

fn engine() -> roostr_engine::Engine {
    roostr_engine::Engine::native().expect("native engine")
}

fn provenance(signer: &str, key_id: i64) -> SharedProvenance {
    SharedProvenance {
        space_id: SPACE_ID.into(),
        key_id,
        signer: signer.into(),
    }
}

/// What the gate tests share: the owner's space with `member` a writer, and
/// `doc`, a note scoped to it.
struct Gate {
    w: World,
    trusted: Value,
    create_doc: Change,
    doc: Value,
    edit: Change,
}

impl Gate {
    async fn new() -> Gate {
        let w = World::new(engine()).await;
        let trusted = w.state(&[w.create_space().await]).await;
        let create_doc = w
            .change(
                "doc",
                json!([
                    {"objectCreate": {"typeKey": "note"}},
                    {"fieldSet": {"key": "channel", "value": {"stringValue": SPACE_ID}}},
                ]),
                &[],
                None,
            )
            .await;
        let doc = w.state(std::slice::from_ref(&create_doc)).await;
        let edit = w
            .change(
                "doc",
                json!([{"fieldSet": {"key": "name", "value": {"stringValue": "safe"}}}]),
                &[],
                None,
            )
            .await;
        Gate {
            w,
            trusted,
            create_doc,
            doc,
            edit,
        }
    }

    fn member(&self) -> SharedProvenance {
        provenance(self.w.member.pubkey(), 1)
    }

    fn owner(&self) -> SharedProvenance {
        provenance(self.w.owner.pubkey(), 1)
    }

    /// The replica's verdict: a domain error is a rejection, not a fault.
    async fn permits_with(
        &self,
        change: &Change,
        existing: Option<&Value>,
        p: &SharedProvenance,
        trusted: &Value,
    ) -> bool {
        let space = SpaceAuthority {
            space_id: SPACE_ID.into(),
            key_id: 1,
            owner: self.w.owner.pubkey().into(),
        };
        match self
            .w
            .engine
            .authorize(
                AuthorizeSubject::Change(change),
                p,
                &space,
                Some(trusted),
                existing,
            )
            .await
        {
            Ok(verdict) => verdict.ok,
            Err(EngineError::Domain(_)) => false,
            Err(error) => panic!("authorize: {error}"),
        }
    }

    async fn permits(
        &self,
        change: &Change,
        existing: Option<&Value>,
        p: &SharedProvenance,
    ) -> bool {
        self.permits_with(change, existing, p, &self.trusted).await
    }
}

/// `value` with every `{"stringValue": from}` under it replaced by `to`.
fn replace_string(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Object(map) => {
            if map.get("stringValue").and_then(Value::as_str) == Some(from) {
                map.insert("stringValue".into(), json!(to));
            }
            map.values_mut().for_each(|v| replace_string(v, from, to));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| replace_string(v, from, to)),
        _ => {}
    }
}

#[test]
fn authority_uses_the_transport_signer_and_held_membership_never_the_inner_author() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        // The edit's inner author is the owner; the member signed it.
        assert!(g.permits(&g.edit, Some(&g.doc), &g.member()).await);
        let stranger = provenance(g.w.stranger.pubkey(), 1);
        assert!(!g.permits(&g.edit, Some(&g.doc), &stranger).await);
        let rotated = provenance(g.w.member.pubkey(), 2);
        assert!(!g.permits(&g.edit, Some(&g.doc), &rotated).await);
        let mut viewer = g.trusted.clone();
        replace_string(&mut viewer["fields"]["members"], "writer", "viewer");
        assert_ne!(viewer, g.trusted, "the member's role was rewritten");
        assert!(
            !g.permits_with(&g.edit, Some(&g.doc), &g.member(), &viewer)
                .await
        );
    });
}

#[test]
fn a_channel_stamp_cannot_steal_existing_personal_or_foreign_objects() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        for scope in ["", "foreign"] {
            let mut target = g.doc.clone();
            target["fields"] = json!({"channel": {"stringValue": scope}});
            assert!(
                !g.permits(&g.create_doc, Some(&target), &g.member()).await,
                "member over scope {scope:?}"
            );
            assert!(
                !g.permits(&g.create_doc, Some(&target), &g.owner()).await,
                "owner over scope {scope:?}"
            );
        }
        assert!(g.permits(&g.create_doc, None, &g.member()).await);
        assert!(!g.permits(&g.edit, None, &g.member()).await);
    });
}

#[test]
fn administrative_fields_control_types_and_channel_deletion_are_the_owners() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        for key in [
            "members",
            "owner",
            "keyId",
            "key",
            "keys",
            "served_by",
            "machine",
            "machine_id",
            "machineId",
        ] {
            let set =
                g.w.change(
                    "doc",
                    json!([{"fieldSet": {"key": key, "value": {"stringValue": "attack"}}}]),
                    &[],
                    None,
                )
                .await;
            assert!(
                !g.permits(&set, Some(&g.doc), &g.member()).await,
                "set {key}"
            );
            let delete =
                g.w.change("doc", json!([{"fieldDelete": {"key": key}}]), &[], None)
                    .await;
            assert!(
                !g.permits(&delete, Some(&g.doc), &g.member()).await,
                "delete {key}"
            );
        }
        for type_key in ["machine", "agent", "program", "typescript", "skill", "peer"] {
            let mut control = g.doc.clone();
            control["typeKey"] = json!(type_key);
            assert!(
                !g.permits(&g.edit, Some(&control), &g.member()).await,
                "member edits {type_key}"
            );
            assert!(
                g.permits(&g.edit, Some(&control), &g.owner()).await,
                "owner edits {type_key}"
            );
        }
        let delete_space =
            g.w.change(SPACE_ID, json!([{"objectDelete": {}}]), &[], None)
                .await;
        assert!(
            !g.permits(&delete_space, Some(&g.trusted), &g.member())
                .await
        );
        let ledger =
            g.w.change(
                "__vanished__",
                json!([
                    {"objectCreate": {"typeKey": "note"}},
                    {"fieldSet": {"key": "channel", "value": {"stringValue": SPACE_ID}}},
                ]),
                &[],
                None,
            )
            .await;
        assert!(!g.permits(&ledger, None, &g.owner()).await);
    });
}

#[test]
fn snapshots_cannot_hide_privilege_changes_drop_protected_fields_or_change_type_or_scope() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        let snapshot = json!({
            "id": "doc", "typeKey": "note",
            "fields": {"channel": {"stringValue": SPACE_ID}},
            "blocks": [], "deleted": false,
        });
        let with = |patch: Value| {
            let mut s = snapshot.clone();
            for (k, v) in patch.as_object().unwrap() {
                s[k] = v.clone();
            }
            s
        };
        let plain =
            g.w.change("doc", json!([]), &[], Some(snapshot.clone()))
                .await;
        assert!(g.permits(&plain, Some(&g.doc), &g.member()).await);
        for mutation in [
            json!({"id": "other"}),
            json!({"typeKey": "agent"}),
            json!({"fields": {}}),
            json!({"fields": {"channel": {"stringValue": SPACE_ID}, "members": {"stringValue": "attack"}}}),
        ] {
            let forged =
                g.w.change("doc", json!([]), &[], Some(with(mutation.clone())))
                    .await;
            assert!(
                !g.permits(&forged, Some(&g.doc), &g.member()).await,
                "snapshot {mutation}"
            );
        }
        let mut served = g.doc.clone();
        served["fields"]["served_by"] = json!({"stringValue": g.w.owner.pubkey()});
        assert!(
            !g.permits(&plain, Some(&served), &g.member()).await,
            "a snapshot cannot drop a protected field"
        );
        let elsewhere =
            g.w.change(
                "doc",
                json!([]),
                &[],
                Some(with(
                    json!({"fields": {"channel": {"stringValue": "foreign"}}}),
                )),
            )
            .await;
        assert!(!g.permits(&elsewhere, Some(&g.doc), &g.owner()).await);
    });
}

#[test]
fn a_membership_removal_takes_effect_before_the_next_item_of_the_same_batch() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let g = Gate::new().await;
        let w = &g.w;
        let create_space = w.create_space().await;
        // A third device of the space: neither stream of its own overlaps the
        // space stream, so both events arrive in one fetch and one batch.
        let device = Replica::memory(keys(5), vec![relay.url.clone()]).await;
        device
            .store
            .add_changes(vec![
                w.record(&create_space).await,
                w.record(&g.create_doc).await,
            ])
            .await
            .unwrap();
        let removal = w
            .change(
                SPACE_ID,
                json!([{"fieldSet": {"key": "members", "value": {"valuesValue": {"items": []}}}}]),
                &[&create_space.id],
                None,
            )
            .await;
        relay.insert(w.change_event(&removal, &w.owner, 100).await);
        relay.insert(w.change_event(&g.edit, &w.member, 101).await);
        device
            .sync
            .set_shared_spaces(vec![w.space()])
            .await
            .unwrap();
        device.sync.start().await.unwrap();
        device.sync.await_first_walk().await;

        assert_eq!(device.sync.stats().events, 2, "both events were received");
        assert_eq!(
            device
                .store
                .changes_for(SPACE_ID.into())
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            device.store.changes_for("doc".into()).await.unwrap().len(),
            1
        );
        assert!(
            !device
                .store
                .is_published(format!("{SPACE_ID}/1/{}", g.edit.id))
                .await
                .unwrap()
        );
        device.sync.stop();
    });
}

#[test]
fn legacy_explicit_default_bytes_import_and_read_back_under_their_own_address() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    block_on(async {
        let g = Gate::new().await;
        let w = &g.w;
        // The canonical encoding with an explicit empty author (field 6)
        // appended, re-addressed over the id-zeroed bytes as old writers did.
        let mut anonymous = g.create_doc.clone();
        anonymous.author = String::new();
        let mut legacy = w.bytes(&anonymous).await;
        legacy.extend_from_slice(&[0x32, 0x00]);
        let mut hasher = Sha256::new();
        hasher.update([0x0a, 0x00]);
        hasher.update(&legacy[34..]);
        let digest: [u8; 32] = hasher.finalize().into();
        legacy[2..34].copy_from_slice(&digest);

        let decoded = w.engine.decode_change(&legacy).await.unwrap();
        assert_eq!(decoded.author, "");
        assert_eq!(decoded.id, hex::encode(digest));
        assert_ne!(w.engine.change_id(&decoded).await.unwrap(), decoded.id);
        assert_ne!(w.engine.encode_change(&decoded).await.unwrap(), legacy);

        let mut tampered = legacy.clone();
        tampered[2] ^= 1;
        relay.insert(w.event(
            kinds::CHANGE,
            &STANDARD.encode(&legacy),
            &w.owner,
            &[],
            None,
            100,
        ));
        relay.insert(w.event(
            kinds::CHANGE,
            &STANDARD.encode(&tampered),
            &w.owner,
            &[],
            None,
            101,
        ));
        let replica = Replica::new(
            w.owner.clone(),
            vec![relay.url.clone()],
            SqliteChangeStore::open(&path).await.unwrap(),
        );
        replica
            .sync
            .set_shared_spaces(vec![w.space()])
            .await
            .unwrap();
        replica.sync.start().await.unwrap();
        replica.sync.await_first_walk().await;
        assert_eq!(replica.sync.stats().events, 2);
        replica.sync.stop();
        replica.store.close().await.unwrap();

        // Read back after a reopen: the exact received bytes, decoded as received.
        let store = SqliteChangeStore::open(&path).await.unwrap();
        let held = store.changes_for("doc".into()).await.unwrap();
        assert_eq!(held.len(), 1, "only the untampered payload imported");
        assert_eq!(held[0].bytes, legacy);
        assert_eq!(held[0].id, decoded.id);
        let stored: Change = serde_json::from_str(&held[0].json).unwrap();
        assert_eq!(stored, decoded);
        assert!(store.changes_for("absent".into()).await.unwrap().is_empty());
    });
}

/// Opens the owner's receive session with the space at `key_id` (and `space_id`).
async fn open_session(w: &World, space_id: &str, key_id: i64, cursor: i64) {
    w.engine
        .session_open(&SessionOpen {
            pk: w.owner.pubkey().into(),
            conversation_key: hex::encode(w.personal_key()),
            secret: Some(w.owner.secret_hex()),
            spaces: vec![session_space(w, space_id, key_id)],
            cursor,
            ..SessionOpen::default()
        })
        .await
        .unwrap();
}

fn session_space(w: &World, space_id: &str, key_id: i64) -> SessionSpace {
    SessionSpace {
        space_id: space_id.into(),
        key_hex: w.key_hex(),
        key_id,
        owner: Some(w.owner.pubkey().into()),
    }
}

/// The change id of the payload `event` completes, if any.
async fn ingest_at(w: &World, event: &roostr_nostr::Event, now_ms: i64) -> Option<String> {
    let result = w.engine.ingest(&relay_event(event), now_ms).await.unwrap();
    result.item.and_then(|item| item.change).map(|c| c.id)
}

async fn ingest(w: &World, event: &roostr_nostr::Event) -> Option<String> {
    ingest_at(w, event, NOW_MS).await
}

#[test]
fn shared_payloads_carry_the_signer_and_personal_ones_need_our_own_signature() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        let w = &g.w;
        open_session(w, SPACE_ID, 1, 0).await;
        let part = w.base64(&g.create_doc).await;
        // Sealed under our personal key, but signed by someone else.
        let foreign = w.event(
            kinds::CHANGE,
            &part,
            &w.stranger,
            &[],
            Some(&w.personal_key()),
            100,
        );
        let result = w
            .engine
            .ingest(&relay_event(&foreign), NOW_MS)
            .await
            .unwrap();
        assert!(result.item.is_none());
        let valid = w.event(kinds::CHANGE, &part, &w.member, &[], None, 100);
        let item = w
            .engine
            .ingest(&relay_event(&valid), NOW_MS)
            .await
            .unwrap()
            .item
            .expect("a whole shared payload");
        assert_eq!(item.provenance, Some(g.member()));
        assert_eq!(item.change.unwrap().id, g.create_doc.id);
        w.engine.session_close().await.unwrap();
    });
}

#[test]
fn parts_never_assemble_across_signers_space_ids_or_key_versions() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        let w = &g.w;
        open_session(w, SPACE_ID, 1, 0).await;
        let (parts, gid) = w.chunks(&g.create_doc).await;
        let part = |i: usize, signer: &roostr_nostr::Keys| {
            w.event(
                kinds::CHANGE,
                &parts[i],
                signer,
                &World::c_tag(&gid, &i.to_string(), "2"),
                None,
                100,
            )
        };
        assert_eq!(ingest(w, &part(0, &w.member)).await, None);
        assert_eq!(ingest(w, &part(1, &w.owner)).await, None, "another signer");
        w.engine
            .session_spaces(&[session_space(w, SPACE_ID, 2)])
            .await
            .unwrap();
        assert_eq!(
            ingest(w, &part(1, &w.member)).await,
            None,
            "another key version"
        );
        w.engine
            .session_spaces(&[session_space(w, "other", 1)])
            .await
            .unwrap();
        assert_eq!(ingest(w, &part(1, &w.member)).await, None, "another space");
        w.engine
            .session_spaces(&[session_space(w, SPACE_ID, 1)])
            .await
            .unwrap();
        assert_eq!(
            ingest(w, &part(1, &w.member)).await,
            Some(g.create_doc.id.clone())
        );
        w.engine.session_close().await.unwrap();
    });
}

#[test]
fn malformed_indices_inconsistent_totals_and_expired_partial_groups_are_refused() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let g = Gate::new().await;
        let w = &g.w;
        open_session(w, SPACE_ID, 1, 0).await;
        let (parts, gid) = w.chunks(&g.create_doc).await;
        let part = |i: usize, index: &str, total: &str| {
            w.event(
                kinds::CHANGE,
                &parts[i],
                &w.member,
                &World::c_tag(&gid, index, total),
                None,
                100,
            )
        };
        for index in ["-1", "2", "1x", "0.5", ""] {
            assert_eq!(
                ingest(w, &part(0, index, "2")).await,
                None,
                "index {index:?}"
            );
        }
        assert_eq!(ingest(w, &part(0, "0", "2")).await, None);
        assert_eq!(
            ingest(w, &part(1, "1", "3")).await,
            None,
            "inconsistent total"
        );
        assert_eq!(
            ingest(w, &part(1, "1", "2")).await,
            None,
            "the group was dropped"
        );
        let later = NOW_MS + GROUP_TTL_MS + 1;
        assert_eq!(
            ingest_at(w, &part(0, "0", "2"), later).await,
            None,
            "the partial group expired"
        );
        assert_eq!(
            ingest_at(w, &part(1, "1", "2"), later).await,
            Some(g.create_doc.id.clone())
        );
        w.engine.session_close().await.unwrap();
    });
}

#[test]
fn active_groups_are_capped_and_expiry_keeps_the_replay_floor() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let w = World::new(engine()).await;
        open_session(&w, SPACE_ID, 1, 200).await;
        let part = |gid: &str| {
            w.event(
                kinds::CHANGE,
                "YQ",
                &w.member,
                &World::c_tag(gid, "0", "2"),
                None,
                100,
            )
        };
        for i in 0..129u32 {
            assert_eq!(ingest(&w, &part(&format!("{i:016x}"))).await, None);
        }
        let state = w.engine.session_state().await.unwrap();
        assert_eq!(state.groups, 128, "open groups are capped");
        assert_eq!(
            state.replay_groups.len(),
            129,
            "an evicted group stays owed"
        );
        // Every obligation reaches back to the parts' second: the cursor floor is 99.
        assert!(state.replay_groups.iter().all(|(_, at)| *at == 100));

        let later = NOW_MS + GROUP_TTL_MS + 1;
        assert_eq!(ingest_at(&w, &part("ffffffffffffffff"), later).await, None);
        let state = w.engine.session_state().await.unwrap();
        assert_eq!(state.groups, 1, "the expired groups closed");
        assert_eq!(
            state.replay_groups.len(),
            130,
            "expiry keeps every obligation"
        );
        assert!(state.replay_groups.iter().all(|(_, at)| *at == 100));
        w.engine.session_close().await.unwrap();
    });
}
