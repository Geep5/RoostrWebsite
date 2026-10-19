//! State sync over a relay (`glonOdin/docs/state-sync.md`, ported from the
//! Svelte host's `scripts/state-sync.test.ts`): bases first (a split base's
//! parts by `#c`), live deltas by `#b`, a window bounded by the newest base
//! of each stream (never another stream's), the vanish ledger read whole,
//! this device's orphans rebased and published with their `b`, live
//! subscriptions carrying bases, and the backend replaying from the current
//! base. Bases are made as a computer makes them: engine `compact_plan` +
//! `seal_base`.
#![cfg(not(target_family = "wasm"))]

#[path = "support/proxy.rs"]
mod proxy;
mod support;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use proxy::RelayProxy;
use roostr_core::Backend;
use roostr_core::types::field_str;
use roostr_engine::{Change, ClassificationBase, Op, SealRequest, SealSpace, StateInputs};
use roostr_nostr::{Event, EventTemplate, Filter, Keys, kinds};
use roostr_replica::local::{FileLocalKv, LocalKv};
use roostr_replica::sync::{RelaySync, SharedSpaceInfo, VANISH_LOG_ID};
use roostr_replica::{RELAYS, ReplicaBackend, ReplicaParts, identity};
use roostr_replica_store::{BaseRecord, ChangeStore, HeldQuery, PendingPublish, SqliteChangeStore};
use serde_json::json;
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, TestHost, block_on, encode, eventually};

/// Base created_at (ms): the bounded window opens a day before it.
const BASE_MS: i64 = 1_760_000_000_000;
const BASE_S: u64 = (BASE_MS / 1000) as u64;
const BOUND_S: u64 = BASE_S - 86_400;
const WAIT: Duration = Duration::from_secs(30);

struct Fixture {
    replica: Replica,
    conversation: String,
}

impl Fixture {
    async fn new(keys: &Keys, relays: Vec<String>) -> Fixture {
        let replica = Replica::memory(keys.clone(), relays).await;
        let conversation =
            hex::encode(roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap());
        Fixture {
            replica,
            conversation,
        }
    }

    async fn change(
        &self,
        object_id: &str,
        ops: Vec<Op>,
        parents: &[&Change],
        timestamp: i64,
    ) -> Change {
        let author = self.replica.keys.pubkey().to_string();
        self.change_by(&author, object_id, ops, parents, timestamp)
            .await
    }

    async fn change_by(
        &self,
        author: &str,
        object_id: &str,
        ops: Vec<Op>,
        parents: &[&Change],
        timestamp: i64,
    ) -> Change {
        let mut change = Change {
            id: String::new(),
            object_id: object_id.into(),
            parent_ids: parents.iter().map(|p| p.id.clone()).collect(),
            ops,
            timestamp,
            author: author.into(),
            snapshot: None,
            b: None,
        };
        change.id = self.replica.engine.change_id(&change).await.unwrap();
        change
    }

    /// The next base over `bases` folding `changes`: (protobuf, hash).
    async fn compact(
        &self,
        bases: &[Vec<u8>],
        changes: Vec<Change>,
        now_ms: i64,
    ) -> (Vec<u8>, String) {
        let plan = self
            .replica
            .engine
            .compact_plan(
                &StateInputs {
                    bases: bases.to_vec(),
                    changes,
                    checkpoint: None,
                    pending: Vec::new(),
                },
                now_ms,
                true,
            )
            .await
            .unwrap();
        assert!(plan.compact, "{}", plan.reason);
        (plan.base.unwrap(), plan.hash.unwrap())
    }

    /// Signed relay events of a sealed base: its 1080 parts, then the 31078.
    async fn base_events(&self, base: &[u8], created_at: u64) -> Vec<Event> {
        let keys = &self.replica.keys;
        let sealed = self
            .replica
            .engine
            .seal_base(
                base,
                &self.conversation,
                Some(&keys.secret_hex()),
                None,
                None,
            )
            .await
            .unwrap();
        sealed
            .parts
            .into_iter()
            .chain([sealed.event])
            .map(|p| {
                keys.sign(EventTemplate {
                    kind: p.kind as u32,
                    created_at,
                    tags: p.tags,
                    content: p.content,
                })
            })
            .collect()
    }

    /// A personal kind-1078 delta as the outbox seals it, `b` when written on a base.
    async fn delta_event(&self, change: &Change, created_at: u64, base: Option<&str>) -> Event {
        let keys = &self.replica.keys;
        let bytes = self.replica.engine.encode_change(change).await.unwrap();
        let sealed = self
            .replica
            .engine
            .seal(&SealRequest {
                change: &bytes,
                conversation_key: &self.conversation,
                secret: Some(&keys.secret_hex()),
                space: None,
                object_id: &change.object_id,
                base,
                nonce: None,
            })
            .await
            .unwrap();
        assert_eq!(sealed.parts.len(), 1);
        let part = sealed.parts.into_iter().next().unwrap();
        keys.sign(EventTemplate {
            kind: kinds::CHANGE,
            created_at,
            tags: part.tags,
            content: part.content,
        })
    }

    /// A shared kind-1078 delta of `space`, as `signer`'s outbox seals it.
    async fn space_delta_event(
        &self,
        signer: &Keys,
        space: &SharedSpaceInfo,
        change: &Change,
        created_at: u64,
    ) -> Event {
        let bytes = self.replica.engine.encode_change(change).await.unwrap();
        let sealed = self
            .replica
            .engine
            .seal(&SealRequest {
                change: &bytes,
                conversation_key: &space.key_hex,
                secret: None,
                space: Some(seal_space(space)),
                object_id: &change.object_id,
                base: None,
                nonce: None,
            })
            .await
            .unwrap();
        assert_eq!(sealed.parts.len(), 1);
        let part = sealed.parts.into_iter().next().unwrap();
        signer.sign(EventTemplate {
            kind: kinds::CHANGE,
            created_at,
            tags: part.tags,
            content: part.content,
        })
    }

    /// Signed relay events of a base sealed into `space` by `signer`.
    async fn space_base_events(
        &self,
        signer: &Keys,
        space: &SharedSpaceInfo,
        base: &[u8],
        created_at: u64,
    ) -> Vec<Event> {
        let sealed = self
            .replica
            .engine
            .seal_base(base, &space.key_hex, None, Some(seal_space(space)), None)
            .await
            .unwrap();
        sealed
            .parts
            .into_iter()
            .chain([sealed.event])
            .map(|p| {
                signer.sign(EventTemplate {
                    kind: p.kind as u32,
                    created_at,
                    tags: p.tags,
                    content: p.content,
                })
            })
            .collect()
    }

    /// A base as the store holds it.
    async fn base_record(&self, base: &[u8]) -> BaseRecord {
        let decoded = self.replica.engine.base_decode(base).await.unwrap();
        BaseRecord {
            object_id: decoded.summary.object_id,
            hash: decoded.summary.hash,
            epoch: decoded.summary.epoch,
            created_at: decoded.summary.created_at,
            prev_base: decoded.summary.prev_base,
            bytes: base.to_vec(),
        }
    }

    /// The history the classification and replay cases share.
    async fn chain(&self) -> Chain {
        let c1 = self
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = self.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        // Epoch 1: the computer folds everything it holds.
        let base1 = self
            .compact(&[], vec![c1.clone(), c2.clone()], BASE_MS)
            .await;
        // Two deltas on base 1: c4 another device's, c3 this device's, never published.
        let c4 = self.change("doc", vec![set_name("v4")], &[&c2], 400).await;
        let c3 = self
            .change("doc", vec![set_note("written offline")], &[&c2], 300)
            .await;
        // The computer saw c4 and a later c5 (whose events it then deleted), never c3.
        let c5 = self.change("doc", vec![set_name("v5")], &[&c4], 500).await;
        let base2 = self
            .compact(
                std::slice::from_ref(&base1.0),
                vec![on(&c4, &base1.1), on(&c5, &base1.1)],
                BASE_MS + 60_000,
            )
            .await;
        let c6 = self.change("doc", vec![set_name("v6")], &[&c5], 600).await;
        let base3 = self
            .compact(
                std::slice::from_ref(&base2.0),
                vec![on(&c6, &base2.1)],
                BASE_MS + 120_000,
            )
            .await;
        Chain {
            c3,
            c4,
            c5,
            base1,
            base2,
            base3,
        }
    }
}

/// c1, c2 folded into base 1; c3 and c4 on base 1; base 2 folds c4 and c5;
/// base 3 folds c6. Bases are (protobuf, hash).
struct Chain {
    c3: Change,
    c4: Change,
    c5: Change,
    base1: (Vec<u8>, String),
    base2: (Vec<u8>, String),
    base3: (Vec<u8>, String),
}

/// `change` as held from an event carrying `["b", base]`.
fn on(change: &Change, base: &str) -> Change {
    let mut change = change.clone();
    change.b = Some(base.into());
    change
}

fn seal_space(space: &SharedSpaceInfo) -> SealSpace<'_> {
    SealSpace {
        key_hex: &space.key_hex,
        space_id: &space.space_id,
        owner: space.owner.as_deref(),
    }
}

fn set_note(text: &str) -> Op {
    Op::FieldSet {
        key: "note".into(),
        value: Some(json!({"stringValue": text})),
    }
}

fn set_name(name: &str) -> Op {
    Op::FieldSet {
        key: "name".into(),
        value: Some(json!({"stringValue": name})),
    }
}

fn create_note() -> Op {
    Op::ObjectCreate {
        type_key: "note".into(),
    }
}

fn shape(f: &Filter) -> String {
    let kinds: Vec<String> = f.kinds.iter().flatten().map(u32::to_string).collect();
    let mut out = kinds.join(",");
    if let Some(b) = f.tags.get(&'b') {
        out += &format!("#b:{}", b.join(","));
    }
    if let Some(h) = f.tags.get(&'h') {
        out += &format!("#h:{}", h.join(","));
    }
    if let Some(since) = f.since {
        out += &format!(">{since}");
    }
    if let Some(until) = f.until {
        out += &format!("<{until}");
    }
    out
}

fn name_of(state: &Option<serde_json::Value>) -> Option<String> {
    state.as_ref()?["fields"]["name"]["stringValue"]
        .as_str()
        .map(str::to_string)
}

/// Which stream a filter reads and how: space filters always carry `#h`
/// (the space tag), a personal one only for the vanish ledger; `based` is
/// the id listing of objects holding a base (their object tags, one per
/// stream key, so always more than one here).
fn stream_shape(f: &Filter) -> String {
    let personal = f.authors.is_some();
    let based = !personal && f.tags.get(&'h').is_some_and(|h| h.len() > 1);
    let kinds: Vec<String> = f.kinds.iter().flatten().map(u32::to_string).collect();
    format!(
        "{}:{}{}{}{}{}",
        if personal {
            "self"
        } else if based {
            "based"
        } else {
            "space"
        },
        kinds.join(","),
        if f.tags.contains_key(&'b') { "#b" } else { "" },
        if personal && f.tags.contains_key(&'h') {
            "#h"
        } else {
            ""
        },
        f.since.map(|s| format!(">{s}")).unwrap_or_default(),
        f.until.map(|u| format!("<{u}")).unwrap_or_default(),
    )
}

/// The kind-1078 NEG-OPENs so far, by stream shape, sorted.
fn change_opens(relay: &TestRelay) -> Vec<String> {
    let mut opens: Vec<String> = relay
        .world()
        .neg_filters
        .iter()
        .filter(|f| f.kinds.iter().flatten().any(|k| *k == kinds::CHANGE))
        .map(stream_shape)
        .collect();
    opens.sort();
    opens
}

/// Another identity's shared space: its events are in the space stream
/// only, never in this key's personal one.
fn other_space(other: &Keys) -> SharedSpaceInfo {
    SharedSpaceInfo {
        space_id: "space".into(),
        key_hex: hex::encode([4u8; 32]),
        key_id: 1,
        writers: vec![other.pubkey().to_string()],
        owner: Some(other.pubkey().to_string()),
    }
}

/// A note of the space named after its id.
fn space_note_ops(object_id: &str) -> Vec<Op> {
    vec![
        create_note(),
        set_name(object_id),
        Op::FieldSet {
            key: "channel".into(),
            value: Some(json!({"stringValue": "space"})),
        },
    ]
}

async fn changes(store: &dyn ChangeStore, object_id: &str) -> Vec<String> {
    store
        .changes_for(object_id.to_string())
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// One more history pass (`resume`, as on foregrounding); its
/// `(complete, reachable)`.
async fn another_pass(sync: &RelaySync, host: &TestHost) -> (bool, bool) {
    let before = host.passes.lock().len();
    sync.resume();
    let ended = eventually(WAIT, async || host.passes.lock().len() > before).await;
    assert!(ended, "the pass ended");
    sync.await_idle().await;
    *host.passes.lock().last().unwrap()
}

#[test]
fn bases_first_then_live_deltas_and_a_bounded_window() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let c1 = f
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = f.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        let (base1, base1_hash) = f.compact(&[], vec![c1.clone(), c2.clone()], BASE_MS).await;
        let c4 = f.change("doc", vec![set_name("v4")], &[&c2], 400).await;
        // A base too big for one event: parts 1..n-1 ride in kind 1080, fetched by #c.
        let big_create = f
            .change(
                "big",
                vec![
                    create_note(),
                    set_name("big"),
                    Op::FieldSet {
                        key: "body".into(),
                        value: Some(json!({"stringValue": "x".repeat(90_000)})),
                    },
                ],
                &[],
                100,
            )
            .await;
        let (big, big_hash) = f.compact(&[], vec![big_create], BASE_MS).await;
        let big_events = f.base_events(&big, BASE_S).await;
        let parts: HashSet<String> = big_events[..big_events.len() - 1]
            .iter()
            .map(|e| e.id.clone())
            .collect();
        assert!(parts.len() > 1);
        assert!(
            big_events[..parts.len()]
                .iter()
                .all(|e| e.kind == kinds::BASE_PART)
        );
        let fresh = f
            .change(
                "fresh",
                vec![create_note(), set_name("new since the base")],
                &[],
                600,
            )
            .await;

        let old = [
            f.delta_event(&c1, 1_700_000_000, None).await,
            f.delta_event(&c2, 1_700_000_100, None).await,
        ];
        let mut kept = f.base_events(&base1, BASE_S).await;
        kept.extend(big_events.clone());
        kept.push(f.delta_event(&c4, 1_750_000_000, Some(&base1_hash)).await);
        kept.push(f.delta_event(&fresh, BASE_S + 100, None).await);
        for event in old.iter().chain(&kept) {
            relay.insert(event.clone());
        }

        let r = &f.replica;
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));

        // Bases, then checkpoints, then the #b deltas with the ledger stream, then the
        // window, then the history below it: ids of the based objects' events, then the rest.
        let ledger = r
            .engine
            .blind_personal(&keys.secret_hex(), "__vanished__")
            .await
            .unwrap();
        let opens: Vec<String> = relay.world().neg_filters.iter().map(shape).collect();
        let mut hashes = [base1_hash.clone(), big_hash.clone()];
        hashes.sort();
        let mut middle = vec![
            format!("1078#b:{}", hashes.join(",")),
            format!("1078#h:{ledger}"),
        ];
        middle.sort();
        let mut seen_middle = opens[2..4].to_vec();
        seen_middle.sort();
        assert_eq!(opens[0..2], ["31078".to_string(), "1079".to_string()]);
        assert_eq!(seen_middle, middle);
        let mut object_tags = Vec::new();
        for id in ["big", "doc"] {
            object_tags.push(
                r.engine
                    .blind_personal(&keys.secret_hex(), id)
                    .await
                    .unwrap(),
            );
        }
        object_tags.sort();
        assert_eq!(
            opens[4..],
            [
                format!("1078>{BOUND_S}"),
                format!("1078#h:{}<{BOUND_S}", object_tags.join(",")),
                format!("1078<{BOUND_S}"),
            ]
        );
        // Parts come by #c, not by id; history older than the window is never fetched.
        let gid = big_events.last().unwrap().tag("c").unwrap().to_string();
        let walks: Vec<String> = relay
            .world()
            .walks
            .iter()
            .filter(|w| w.tags.contains_key(&'c'))
            .map(|w| format!("{}#c:{}", shape(w), w.tags[&'c'].join(",")))
            .collect();
        assert_eq!(walks, [format!("1080#c:{gid}")]);
        let fetched: HashSet<String> = relay.world().id_fetches.iter().flatten().cloned().collect();
        let expected: HashSet<String> = kept
            .iter()
            .filter(|e| !parts.contains(&e.id))
            .map(|e| e.id.clone())
            .collect();
        assert_eq!(fetched, expected);

        let stats = r.sync.stats();
        assert_eq!(stats.bases, 2);
        let mut based: Vec<String> = r.store.current_bases().await.unwrap().into_keys().collect();
        based.sort();
        assert_eq!(based, ["big", "doc"]);
        let doc: Vec<(String, Option<String>)> = r
            .store
            .changes_for("doc".into())
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.b))
            .collect();
        assert_eq!(doc, [(c4.id.clone(), Some(base1_hash.clone()))]);
        assert_eq!(r.store.changes_for("fresh".into()).await.unwrap().len(), 1);
        assert_eq!(name_of(&r.state("doc").await).as_deref(), Some("v4"));
        let big_state = r.state("big").await.unwrap();
        assert_eq!(
            big_state["fields"]["body"]["stringValue"]
                .as_str()
                .unwrap()
                .len(),
            90_000
        );
        // The #b set is held, scoped to the personal stream; the base's stream floor too.
        let held_b = r
            .store
            .held_events(
                String::new(),
                HeldQuery {
                    kinds: vec![kinds::CHANGE],
                    b: Some(vec![base1_hash.clone()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(held_b.len(), 1);
        assert_eq!(
            r.store.newest_base_by_stream().await.unwrap().get(""),
            Some(&BASE_MS)
        );
        // The #b set is held: a second pass fetches nothing and asks for no parts.
        let (fetches, walks) = {
            let world = relay.world();
            (world.id_fetches.len(), world.walks.len())
        };
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        let world = relay.world();
        assert_eq!(world.id_fetches.len(), fetches);
        assert!(
            world.walks[walks..]
                .iter()
                .all(|w| !w.tags.contains_key(&'c'))
        );
        drop(world);
        r.sync.stop();
    });
}

#[test]
fn a_new_base_rebases_this_devices_orphan_and_publishes_it_with_b() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let c1 = f
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = f.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        let (base1, base1_hash) = f.compact(&[], vec![c1.clone(), c2.clone()], BASE_MS).await;
        let c4 = f.change("doc", vec![set_name("v4")], &[&c2], 400).await;
        let c3 = f
            .change(
                "doc",
                vec![Op::FieldSet {
                    key: "note".into(),
                    value: Some(json!({"stringValue": "written offline"})),
                }],
                &[&c2],
                300,
            )
            .await;
        let c5 = f.change("doc", vec![set_name("v5")], &[&c4], 500).await;
        let mut c4b = c4.clone();
        c4b.b = Some(base1_hash.clone());
        let mut c5b = c5.clone();
        c5b.b = Some(base1_hash.clone());
        let (base2, base2_hash) = f
            .compact(
                std::slice::from_ref(&base1),
                vec![c4b, c5b],
                BASE_MS + 60_000,
            )
            .await;

        for event in f.base_events(&base1, BASE_S).await {
            relay.insert(event);
        }
        relay.insert(f.delta_event(&c4, 1_750_000_000, Some(&base1_hash)).await);
        let r = &f.replica;
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(
            r.store
                .current_base("doc".into())
                .await
                .unwrap()
                .unwrap()
                .hash,
            base1_hash
        );

        // This device wrote c3 on base 1 and never published it (as the backend commits: b + obligation).
        let (mut record, bytes) = encode(&r.engine, c3.clone()).await;
        record.b = Some(base1_hash.clone());
        r.store
            .add_local_change(
                record,
                PendingPublish {
                    key: c3.id.clone(),
                    object_id: "doc".into(),
                    change_id: c3.id.clone(),
                    bytes,
                    space_id: None,
                    key_id: None,
                    base: Some(base1_hash.clone()),
                    events: None,
                },
            )
            .await
            .unwrap();

        // Another computer compacts again: base 2 arrives live.
        // Published now: live subscriptions of a fresh replica start shortly before now.
        for event in f.base_events(&base2, roostr_nostr::now_s()).await {
            relay.insert(event);
        }
        let rebased = eventually(WAIT, async || r.sync.stats().rebased == 1).await;
        assert!(rebased, "the orphan was rebased");
        assert_eq!(
            r.store
                .current_base("doc".into())
                .await
                .unwrap()
                .unwrap()
                .hash,
            base2_hash
        );
        let inputs = r.store.object_inputs("doc".into()).await.unwrap();
        assert!(
            inputs.changes.iter().all(|c| c.id != c3.id),
            "the orphan is never replayed again"
        );
        assert_eq!(inputs.count, 3, "c4, c3 (marked) and its rebase");
        let new = inputs.changes.iter().find(|c| c.id != c4.id).unwrap();
        let change: Change = serde_json::from_str(&new.json).unwrap();
        assert_eq!(change.parent_ids, std::slice::from_ref(&c5.id));
        assert_eq!(change.timestamp, c3.timestamp);
        assert_eq!(change.author, c3.author);
        assert_eq!(change.ops, c3.ops);
        assert_eq!(new.b.as_deref(), Some(base2_hash.as_str()));
        let state = r.state("doc").await;
        assert_eq!(name_of(&state).as_deref(), Some("v5"));
        assert_eq!(
            state.unwrap()["fields"]["note"]["stringValue"],
            "written offline"
        );

        // Published carrying ["b", base2].
        let published = eventually(WAIT, async || {
            r.store.is_published(new.id.clone()).await.unwrap()
        })
        .await;
        assert!(published);
        let on_relay = relay
            .world()
            .events
            .iter()
            .any(|e| e.kind == kinds::CHANGE && e.tag("b") == Some(base2_hash.as_str()));
        assert!(on_relay, "the rebased change went out with its b");

        // The same base again (another relay, another event): nothing left to rebase.
        let again = f.base_events(&base2, roostr_nostr::now_s() + 5).await;
        let again_id = again.last().unwrap().id.clone();
        for event in again {
            relay.insert(event);
        }
        let held_again = eventually(WAIT, async || {
            let held = r
                .store
                .held_events(
                    String::new(),
                    HeldQuery {
                        kinds: vec![kinds::BASE],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            held.iter().any(|h| h.id == again_id)
        })
        .await;
        assert!(held_again, "the repeated base was handled");
        r.sync.await_idle().await;
        let stats = r.sync.stats();
        assert_eq!((stats.rebased, stats.bases), (1, 2));
        r.sync.stop();
    });
}

#[test]
fn a_pending_id_makes_this_devices_own_delta_on_an_unknown_base_an_orphan() {
    let _turn = ENGINE_TURN.lock();
    let keys = Keys::generate();
    block_on(async {
        let f = Fixture::new(&keys, Vec::new()).await;
        let chain = f.chain().await;
        let engine = &f.replica.engine;
        // Only the newest base held: base 2, between c3's base 1 and base 3, never
        // reached this device - it may have folded c3, so a delta of unknown fate is
        // neither replayed nor rebased.
        let input = StateInputs {
            bases: vec![chain.base3.0.clone()],
            changes: vec![on(&chain.c3, &chain.base1.1)],
            checkpoint: None,
            pending: Vec::new(),
        };
        let classified = engine.classify(&input).await.unwrap();
        assert_eq!(
            classified.base,
            Some(ClassificationBase {
                hash: chain.base3.1.clone(),
                epoch: 3
            })
        );
        assert!(classified.orphan.is_empty());
        assert_eq!(classified.stale, std::slice::from_ref(&chain.c3.id));
        // This device never published it: no compactor can have seen it.
        let pending = StateInputs {
            pending: vec![chain.c3.id.clone()],
            ..input.clone()
        };
        let classified = engine.classify(&pending).await.unwrap();
        assert_eq!(classified.orphan, std::slice::from_ref(&chain.c3.id));
        assert!(classified.stale.is_empty());
        let state = engine.replay_from_base(&input).await.unwrap();
        assert_eq!(name_of(&state).as_deref(), Some("v6"));
        assert!(state.unwrap()["fields"].get("note").is_none());
    });
}

#[test]
fn the_vanish_ledger_is_read_whole_even_when_older_than_the_window() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        // A fresh replica of a compacted vault must still load __vanished__, whose
        // history predates the newest base by more than a day.
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let c1 = f
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = f.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        let (base1, _) = f.compact(&[], vec![c1.clone(), c2], BASE_MS).await;
        let ledger_ops = vec![
            Op::ObjectCreate {
                type_key: "vanish_log".into(),
            },
            set_name("Vanished objects"),
        ];
        let ledger = f.change(VANISH_LOG_ID, ledger_ops, &[], 50).await;
        for event in f.base_events(&base1, BASE_S).await {
            relay.insert(event);
        }
        relay.insert(f.delta_event(&ledger, 1_600_000_000, None).await);
        relay.insert(f.delta_event(&c1, 1_700_000_000, None).await);
        let r = &f.replica;
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        assert_eq!(
            changes(&*r.store, VANISH_LOG_ID).await,
            std::slice::from_ref(&ledger.id)
        );
        // Pre-migration history of a based object stays unfetched.
        assert!(changes(&*r.store, "doc").await.is_empty());
        r.sync.stop();
    });
}

#[test]
fn a_space_holding_no_base_is_walked_whole_while_the_personal_stream_is_bounded() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    let other = Keys::generate();
    block_on(async {
        // The newest base of ANY stream used to bound every stream, so an unbased
        // object whose events all predate that window was never fetched.
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let space = other_space(&other);
        let c1 = f
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = f.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        let (base1, _) = f.compact(&[], vec![c1.clone(), c2], BASE_MS).await;
        let old = f
            .change_by(other.pubkey(), "sold", space_note_ops("sold"), &[], 50)
            .await;
        for event in f.base_events(&base1, BASE_S).await {
            relay.insert(event);
        }
        relay.insert(f.delta_event(&c1, 1_700_000_000, None).await);
        relay.insert(
            f.space_delta_event(&other, &space, &old, 1_600_000_000)
                .await,
        );
        let r = &f.replica;
        r.sync.set_shared_spaces(vec![space]).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        let mut expected = vec![
            "self:1078#b".to_string(),
            "self:1078#h".to_string(),
            format!("self:1078>{BOUND_S}"),
            "space:1078".to_string(),
            format!("based:1078<{BOUND_S}"),
            format!("self:1078<{BOUND_S}"),
        ];
        expected.sort();
        assert_eq!(change_opens(&relay), expected);
        assert_eq!(
            changes(&*r.store, "sold").await,
            std::slice::from_ref(&old.id)
        );
        // The personal stream stays bounded: pre-migration history of its based object
        // is listed below the window, never fetched.
        assert!(changes(&*r.store, "doc").await.is_empty());
        assert_eq!(
            r.store.newest_base_by_stream().await.unwrap(),
            HashMap::from([(String::new(), BASE_MS)])
        );
        r.sync.stop();
    });
}

#[test]
fn a_personal_stream_holding_no_base_is_walked_whole_while_a_based_space_is_bounded_and_swept() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    let other = Keys::generate();
    block_on(async {
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let space = other_space(&other);
        let mine = f
            .change("mine", vec![create_note(), set_name("mine")], &[], 40)
            .await;
        let by_other = async |id: &str, timestamp: i64| {
            f.change_by(other.pubkey(), id, space_note_ops(id), &[], timestamp)
                .await
        };
        let based = by_other("sdoc", 60).await;
        let (space_base, space_base_hash) = f.compact(&[], vec![based.clone()], BASE_MS).await;
        let unbased = by_other("sold", 70).await;
        let recent = by_other("snew", 80).await;
        relay.insert(f.delta_event(&mine, 1_600_000_000, None).await);
        for event in f
            .space_base_events(&other, &space, &space_base, BASE_S)
            .await
        {
            relay.insert(event);
        }
        for (change, created_at) in [
            (&based, 1_650_000_000),
            (&unbased, 1_650_000_100),
            (&recent, BASE_S + 100),
        ] {
            relay.insert(
                f.space_delta_event(&other, &space, change, created_at)
                    .await,
            );
        }
        let r = &f.replica;
        let tag = r
            .engine
            .blind_space(&space.key_hex, "space:space")
            .await
            .unwrap();
        r.sync.set_shared_spaces(vec![space]).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        // No ledger reconcile: the whole personal walk already covers it. Below the
        // space's window: the based object's ids, then the space stream without them.
        let mut expected = vec![
            "self:1078".to_string(),
            "space:1078#b".to_string(),
            format!("space:1078>{BOUND_S}"),
            format!("based:1078<{BOUND_S}"),
            format!("space:1078<{BOUND_S}"),
        ];
        expected.sort();
        assert_eq!(change_opens(&relay), expected);
        let current = r.store.current_base("sdoc".into()).await.unwrap();
        assert_eq!(current.map(|b| b.hash), Some(space_base_hash));
        assert_eq!(
            changes(&*r.store, "mine").await,
            std::slice::from_ref(&mine.id)
        );
        assert_eq!(
            changes(&*r.store, "snew").await,
            std::slice::from_ref(&recent.id)
        );
        // Older than the space's window and without a base: the sweep below the
        // window fetches it; the based object's history there stays unfetched.
        assert_eq!(
            changes(&*r.store, "sold").await,
            std::slice::from_ref(&unbased.id)
        );
        assert!(changes(&*r.store, "sdoc").await.is_empty());
        assert_eq!(
            r.store.newest_base_by_stream().await.unwrap(),
            HashMap::from([(tag, BASE_MS)])
        );
        r.sync.stop();
    });
}

/// A vault whose computer based one object but not two others, both created
/// long before its newest base: what a fresh device must still show.
struct Unbased {
    /// Events of the based object, older than the window.
    covered: Vec<Event>,
    /// Events of the unbased objects, older than the window.
    unbased: Vec<Event>,
}

impl Fixture {
    async fn unbased_vault(&self, relay: &TestRelay) -> Unbased {
        let c1 = self
            .change("doc", vec![create_note(), set_name("v1")], &[], 100)
            .await;
        let c2 = self.change("doc", vec![set_name("v2")], &[&c1], 200).await;
        let (base1, _) = self
            .compact(&[], vec![c1.clone(), c2.clone()], BASE_MS)
            .await;
        let a1 = self
            .change("agent", vec![create_note(), set_name("agent")], &[], 300)
            .await;
        let a2 = self
            .change("agent", vec![set_name("agent v2")], &[&a1], 400)
            .await;
        // Created below the window, edited inside it.
        let p1 = self
            .change("pinned", vec![create_note(), set_name("pinned")], &[], 500)
            .await;
        let p2 = self
            .change("pinned", vec![set_note("edited")], &[&p1], 600)
            .await;
        for event in self.base_events(&base1, BASE_S).await {
            relay.insert(event);
        }
        let covered = vec![
            self.delta_event(&c1, 1_700_000_000, None).await,
            self.delta_event(&c2, 1_700_000_100, None).await,
        ];
        let unbased = vec![
            self.delta_event(&a1, 1_700_000_200, None).await,
            self.delta_event(&a2, 1_700_000_300, None).await,
            self.delta_event(&p1, 1_700_000_400, None).await,
        ];
        for event in covered.iter().chain(&unbased) {
            relay.insert(event.clone());
        }
        relay.insert(self.delta_event(&p2, BASE_S + 50, None).await);
        Unbased { covered, unbased }
    }

    /// Every object of [`Fixture::unbased_vault`] in its newest state, and
    /// only the unbased objects' old history fetched.
    async fn assert_unbased_held(&self, relay: &TestRelay, vault: &Unbased) {
        let r = &self.replica;
        assert_eq!(
            name_of(&r.state("agent").await).as_deref(),
            Some("agent v2")
        );
        let pinned = r.state("pinned").await.unwrap();
        assert_eq!(pinned["fields"]["name"]["stringValue"], "pinned");
        assert_eq!(pinned["fields"]["note"]["stringValue"], "edited");
        assert_eq!(name_of(&r.state("doc").await).as_deref(), Some("v2"));
        let fetched: HashSet<String> = relay.world().id_fetches.iter().flatten().cloned().collect();
        assert!(vault.unbased.iter().all(|e| fetched.contains(&e.id)));
        assert!(vault.covered.iter().all(|e| !fetched.contains(&e.id)));
        assert_eq!(
            r.store.meta("unbased-swept".into()).await.unwrap(),
            Some(format!("{{\"\":{BOUND_S}}}"))
        );
    }
}

#[test]
fn an_object_no_base_covers_is_swept_from_below_the_window_once() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        // Field report: a fresh iPhone missed an agent and a pinned object the
        // browser showed. The browser had walked the whole stream before any base
        // existed; the phone read a day below the newest base, and no computer had
        // based those objects.
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let vault = f.unbased_vault(&relay).await;
        let r = &f.replica;
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        f.assert_unbased_held(&relay, &vault).await;
        // Swept once: the next pass lists nothing below the window again.
        let (opens, fetches) = {
            let world = relay.world();
            (world.neg_filters.len(), world.id_fetches.len())
        };
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        let world = relay.world();
        assert!(world.neg_filters[opens..].iter().all(|f| f.until.is_none()));
        assert_eq!(world.id_fetches.len(), fetches);
        drop(world);
        r.sync.stop();
    });
}

#[test]
fn a_device_bootstrapped_before_the_sweep_fetches_what_its_window_missed() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        // An install that finished its first walk without the sweep heals on the
        // next pass (launch or foreground): nothing to log out of or reset.
        let f = Fixture::new(&keys, vec![relay.url.clone()]).await;
        let vault = f.unbased_vault(&relay).await;
        let r = &f.replica;
        r.store.set_bootstrapped().await.unwrap();
        r.store.set_cursor(BASE_S as i64 + 50, None).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        f.assert_unbased_held(&relay, &vault).await;
        r.sync.stop();
    });
}

#[test]
fn live_subscriptions_carry_bases_and_their_parts() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let proxy = RelayProxy::start(&relay.url);
    let keys = Keys::generate();
    block_on(async {
        let f = Fixture::new(&keys, vec![proxy.url.clone()]).await;
        let r = &f.replica;
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        // The live subscription is the one REQ carrying several filters.
        let live = async || {
            proxy
                .rules()
                .reqs
                .iter()
                .filter(|filters| filters.len() > 1)
                .flatten()
                .find(|f| f.authors.is_some())
                .cloned()
        };
        assert!(eventually(WAIT, async || live().await.is_some()).await);
        assert_eq!(
            live().await.unwrap().kinds,
            Some(vec![
                kinds::CHANGE,
                kinds::CHECKPOINT,
                kinds::BASE,
                kinds::BASE_PART
            ])
        );
        r.sync.stop();
    });
}

/// A replica backend over `dir` with no relays, signed in as `keys`.
async fn offline_backend(
    dir: &std::path::Path,
    keys: &Keys,
    store: Arc<SqliteChangeStore>,
) -> ReplicaBackend {
    let kv = Arc::new(FileLocalKv::open(dir.join("local.json")).unwrap());
    kv.set(RELAYS, Some("[]"));
    identity::save(&*kv, &keys.secret_hex()).unwrap();
    ReplicaBackend::new(ReplicaParts {
        engine: roostr_engine::Engine::native().unwrap(),
        store,
        kv,
        label: "test replica".into(),
    })
}

async fn name_in(backend: &ReplicaBackend, id: &str) -> String {
    let object = backend.object(id).await.unwrap();
    field_str(&object.fields, "name").to_string()
}

#[test]
fn the_replay_memo_follows_the_base_and_local_writes_build_on_the_current_base() {
    let _turn = ENGINE_TURN.lock();
    let keys = Keys::generate();
    let dir = tempfile::tempdir().unwrap();
    block_on(async {
        let f = Fixture::new(&keys, Vec::new()).await;
        let chain = f.chain().await;
        let (base1, base2) = (&chain.base1, &chain.base2);
        let store = Arc::new(
            SqliteChangeStore::open(dir.path().join("replica.sqlite"))
                .await
                .unwrap(),
        );
        store
            .put_base(f.base_record(&base1.0).await, vec![String::new()])
            .await
            .unwrap();
        let (mut c4, _) = encode(&f.replica.engine, chain.c4.clone()).await;
        let held = on(&serde_json::from_str(&c4.json).unwrap(), &base1.1);
        c4.json = serde_json::to_string(&held).unwrap();
        c4.b = held.b;
        store.add_changes(vec![c4]).await.unwrap();

        let backend = offline_backend(dir.path(), &keys, store.clone()).await;
        assert_eq!(name_in(&backend, "doc").await, "v4");
        // Held from a base: queried from its replayed JSON, not the corpus.
        let found = backend
            .query(&json!({"filters": [], "limit": 10}))
            .await
            .unwrap();
        let names: Vec<Option<&str>> = found.records.iter().map(|r| r.name.as_deref()).collect();
        assert_eq!(names, [Some("v4")]);
        backend.stop();
        drop(backend);

        // Same change count, same (absent) checkpoint: only the base moved. The
        // replay memo of base 1 must not serve the next load.
        store
            .put_base(f.base_record(&base2.0).await, vec![String::new()])
            .await
            .unwrap();
        let backend = offline_backend(dir.path(), &keys, store.clone()).await;
        assert_eq!(name_in(&backend, "doc").await, "v5");

        let params = json!({"object_id": "doc", "key": "name", "value": {"stringValue": "v6"}});
        backend
            .mutate("set_field", params.as_object().cloned().unwrap())
            .await
            .unwrap();
        let inputs = store.object_inputs("doc".into()).await.unwrap();
        let local = inputs.changes.iter().find(|c| c.id != chain.c4.id).unwrap();
        let change: Change = serde_json::from_str(&local.json).unwrap();
        assert_eq!(change.parent_ids, std::slice::from_ref(&chain.c5.id));
        assert_eq!(local.b.as_deref(), Some(base2.1.as_str()));
        let owed = store.pending(local.id.clone()).await.unwrap().unwrap();
        assert_eq!(owed.base.as_deref(), Some(base2.1.as_str()));
        assert_eq!(name_in(&backend, "doc").await, "v6");
        backend.stop();
    });
}
