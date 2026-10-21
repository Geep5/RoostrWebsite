//! The history pass over NIP-77 (the Svelte host's
//! `scripts/sync-negentropy.test.ts`): exactly the needed ids fetched once,
//! in batches; progress per pass; a withheld id or a failed import keeps
//! history incomplete until a later pass imports it; chunk parts held only
//! once their group imports; the paged walk when a relay refuses NIP-77;
//! and resume reconciling what live never carries.
#![cfg(not(target_family = "wasm"))]

#[path = "support/hooked_store.rs"]
mod hooked_store;
#[path = "support/proxy.rs"]
mod proxy;
mod support;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hooked_store::HookedStore;
use proxy::RelayProxy;
use roostr_engine::{Change, Engine, SealRequest, SealSpace};
use roostr_nostr::spacekeys::{InMemorySpaceKeyring, KeyFile};
use roostr_nostr::{Event, EventTemplate, Filter, Keys, kinds};
use roostr_replica::local::{InMemoryLocalKv, LocalKv};
use roostr_replica::sync::{
    FETCH_BATCH, Keyring, PAGE_LIMIT, Phase, RelaySync, SharedSpaceInfo, SyncHost,
};
use roostr_replica_store::{ChangeStore, HeldQuery, SqliteChangeStore, StoreError};
use serde_json::json;
use support::relay::TestRelay;
use support::{ENGINE_TURN, Replica, TestHost, block_on, create, eventually};

const WAIT: Duration = Duration::from_secs(30);

/// This identity's own shared space (key `04…04`).
fn own_space(keys: &Keys) -> SharedSpaceInfo {
    SharedSpaceInfo {
        space_id: "space".into(),
        key_hex: hex::encode([4u8; 32]),
        key_id: 1,
        writers: vec![keys.pubkey().to_string()],
        owner: None,
    }
}

/// The space stream's `#h` value.
async fn space_tag(engine: &Engine, space: &SharedSpaceInfo) -> String {
    engine
        .blind_space(&space.key_hex, &format!("space:{}", space.space_id))
        .await
        .unwrap()
}

/// A note named after its id, with `extra` fields.
fn note(keys: &Keys, object_id: &str, extra: &[(&str, serde_json::Value)]) -> Change {
    create(keys, object_id, "note", object_id, extra, 100)
}

/// `change` sealed and signed as the outbox would: personal, or into
/// `space` (this identity's own) under its key.
async fn seal(
    engine: &Engine,
    keys: &Keys,
    mut change: Change,
    created_at: u64,
    space: Option<&SharedSpaceInfo>,
) -> Vec<Event> {
    change.id = engine.change_id(&change).await.unwrap();
    let bytes = engine.encode_change(&change).await.unwrap();
    let personal_key =
        hex::encode(roostr_nostr::nip44::conversation_key(keys, keys.pubkey()).unwrap());
    let secret = keys.secret_hex();
    let sealed = engine
        .seal(&SealRequest {
            change: &bytes,
            conversation_key: space.map_or(personal_key.as_str(), |s| s.key_hex.as_str()),
            secret: space.is_none().then_some(secret.as_str()),
            space: space.map(|s| SealSpace {
                key_hex: &s.key_hex,
                space_id: &s.space_id,
                owner: Some(keys.pubkey()),
            }),
            object_id: &change.object_id,
            base: None,
            nonce: None,
        })
        .await
        .unwrap();
    sealed
        .parts
        .into_iter()
        .map(|part| {
            keys.sign(EventTemplate {
                kind: kinds::CHANGE,
                created_at,
                tags: part.tags,
                content: part.content,
            })
        })
        .collect()
}

/// A change small enough for one event.
async fn single(engine: &Engine, keys: &Keys, change: Change, created_at: u64) -> Event {
    let mut events = seal(engine, keys, change, created_at, None).await;
    assert_eq!(events.len(), 1);
    events.pop().unwrap()
}

/// Kinds, and `#h` when the filter carries one.
fn kinds_shape(f: &Filter) -> String {
    let kinds: Vec<String> = f.kinds.iter().flatten().map(u32::to_string).collect();
    let h = if f.tags.contains_key(&'h') { "#h" } else { "" };
    format!("{}{h}", kinds.join(","))
}

/// Shapes in phases of two (self and space stream, covered concurrently).
fn phases(shapes: Vec<String>) -> Vec<Vec<String>> {
    shapes
        .chunks(2)
        .map(|pair| {
            let mut pair = pair.to_vec();
            pair.sort();
            pair
        })
        .collect()
}

const STREAM_PHASES: [[&str; 2]; 3] = [
    ["31078", "31078#h"],
    ["1079", "1079,5#h"],
    ["1078", "1078#h"],
];

/// History walks: one-filter `REQ`s other than id fetches, gift wraps and
/// the keyring backup.
fn dag_walks(relay: &TestRelay) -> Vec<Filter> {
    relay
        .world()
        .walks
        .iter()
        .filter(|f| {
            !f.kinds
                .iter()
                .flatten()
                .any(|k| *k == kinds::GIFT_WRAP || *k == kinds::KEYRING)
        })
        .cloned()
        .collect()
}

fn fetched(relay: &TestRelay, from: usize) -> Vec<String> {
    relay.world().id_fetches[from..]
        .iter()
        .flatten()
        .cloned()
        .collect()
}

/// Held kind-1078 ids of one stream scope.
async fn held(store: &dyn ChangeStore, scope: &str) -> Vec<String> {
    store
        .held_events(
            scope.to_string(),
            HeldQuery {
                kinds: vec![kinds::CHANGE],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.id)
        .collect()
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

fn ids(events: &[Event]) -> HashSet<String> {
    events.iter().map(|e| e.id.clone()).collect()
}

#[test]
fn a_fresh_store_fetches_exactly_the_needed_ids_and_a_second_pass_nothing() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let proxy = RelayProxy::start(&relay.url);
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![proxy.url.clone()]).await;
        let space = own_space(&keys);
        let tag = space_tag(&r.engine, &space).await;
        let mut events = Vec::new();
        for i in 0..230u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            events.push(single(&r.engine, &keys, change, 1000 + (i >> 2)).await);
        }
        // Our own space event sits in both the self stream and the space stream.
        let channel = [("channel", json!({"stringValue": "space"}))];
        let shared = seal(
            &r.engine,
            &keys,
            note(&keys, "sdoc", &channel),
            2000,
            Some(&space),
        )
        .await;
        assert_eq!(shared.len(), 1);
        events.extend(shared.clone());
        for event in &events {
            relay.insert(event.clone());
        }
        r.sync.set_shared_spaces(vec![space]).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));

        // Bases, checkpoints, then changes: self + space each, whole.
        let filters = relay.world().neg_filters.clone();
        assert_eq!(
            phases(filters.iter().map(kinds_shape).collect()),
            STREAM_PHASES
        );
        assert!(
            filters
                .iter()
                .all(|f| f.since.is_none() && f.limit.is_none())
        );
        assert!(dag_walks(&relay).is_empty());
        // Each id once, though `shared` is in two streams; in batches.
        let all = fetched(&relay, 0);
        assert_eq!(all.len(), events.len());
        assert_eq!(all.into_iter().collect::<HashSet<_>>(), ids(&events));
        assert!(
            relay
                .world()
                .id_fetches
                .iter()
                .all(|batch| batch.len() <= FETCH_BATCH)
        );
        let closed = eventually(WAIT, async || proxy.rules().neg_closes == 6).await;
        assert!(closed, "{} NEG-CLOSEs", proxy.rules().neg_closes);
        for i in (0..230).step_by(57) {
            assert_eq!(changes(&*r.store, &format!("doc{i}")).await.len(), 1);
        }
        assert_eq!(changes(&*r.store, "sdoc").await.len(), 1);
        // Held per stream: the shared event under both scopes.
        assert_eq!(held(&*r.store, "").await.len(), 231);
        assert_eq!(held(&*r.store, &tag).await, [shared[0].id.clone()]);

        let before = relay.world().id_fetches.len();
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        assert!(fetched(&relay, before).is_empty());
        r.sync.stop();
    });
}

#[test]
fn a_pass_reports_received_and_expected_and_the_next_pass_starts_over() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![relay.url.clone()]).await;
        for i in 0..5u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            relay.insert(single(&r.engine, &keys, change, 1000 + i).await);
        }
        let progress = |host: &TestHost| -> Vec<(u64, Option<u64>)> {
            host.statuses
                .lock()
                .iter()
                .filter_map(|s| Some((s.received?, s.expected)))
                .collect()
        };
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        let during = progress(&r.host);
        assert_eq!(during.last(), Some(&(5, Some(5))));
        assert!(during.windows(2).all(|w| w[0].0 <= w[1].0), "monotonic");
        // Between passes there is no pass progress.
        let last = r.host.statuses.lock().last().cloned().unwrap();
        assert_eq!((last.received, last.expected), (None, None));

        // Older than the cursor: only the next pass can carry it.
        let late = note(&keys, "doc5", &[]);
        relay.insert(single(&r.engine, &keys, late, 900).await);
        r.host.statuses.lock().clear();
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        assert_eq!(progress(&r.host).last(), Some(&(1, Some(1))));
        r.sync.stop();
    });
}

#[test]
fn a_needed_id_the_relay_does_not_return_keeps_history_incomplete_until_imported() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let proxy = RelayProxy::start(&relay.url);
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![proxy.url.clone()]).await;
        let mut events = Vec::new();
        for i in 0..3u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            events.push(single(&r.engine, &keys, change, 1000 + i).await);
        }
        events.iter().for_each(|e| relay.insert(e.clone()));
        proxy.rules().withhold.insert(events[1].id.clone());
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(false, true)));
        let reported = r.host.statuses.lock().iter().any(|s| {
            s.detail
                .as_deref()
                .is_some_and(|d| d.contains("1 reconciled events not returned"))
        });
        assert!(reported);
        assert_eq!(held(&*r.store, "").await.len(), 2);

        proxy.rules().withhold.clear();
        let before = relay.world().id_fetches.len();
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        assert_eq!(fetched(&relay, before), [events[1].id.clone()]);
        assert_eq!(changes(&*r.store, "doc1").await.len(), 1);
        r.sync.stop();
    });
}

#[test]
fn a_batch_whose_import_fails_holds_nothing_and_is_fetched_again() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let engine = Engine::native().unwrap();
        let mut events = Vec::new();
        for i in 0..2u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            events.push(single(&engine, &keys, change, 1000 + i).await);
        }
        events.iter().for_each(|e| relay.insert(e.clone()));
        let store = Arc::new(HookedStore::new(
            SqliteChangeStore::in_memory().await.unwrap(),
        ));
        // The next page import fails with a full disk.
        let failed = AtomicBool::new(false);
        store.on_import(move |_| {
            let fail = !failed.swap(true, Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(StoreError::Backend("disk full".into()))
                } else {
                    Ok(())
                }
            })
        });
        let host = Arc::new(TestHost::default());
        let sync = RelaySync::new(
            engine,
            store.clone() as Arc<dyn ChangeStore>,
            keys.clone(),
            vec![relay.url.clone()],
            Arc::new(InMemorySpaceKeyring::new(KeyFile::default())) as Arc<dyn Keyring>,
            Arc::new(InMemoryLocalKv::new()) as Arc<dyn LocalKv>,
            host.clone() as Arc<dyn SyncHost>,
        );
        sync.start().await.unwrap();
        sync.await_first_walk().await;
        let failed = host.statuses.lock().iter().any(|s| {
            s.phase == Phase::Error && s.detail.as_deref().is_some_and(|d| d.contains("disk full"))
        });
        assert!(failed, "the pass failed with the store error");
        assert!(!host.passes.lock().iter().any(|(complete, _)| *complete));
        assert!(held(&*store, "").await.is_empty());

        let before = relay.world().id_fetches.len();
        assert_eq!(another_pass(&sync, &host).await, (true, true));
        let again: HashSet<String> = fetched(&relay, before).into_iter().collect();
        assert_eq!(again, ids(&events));
        sync.stop();
    });
}

#[test]
fn chunk_parts_are_held_only_once_their_group_imports() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![relay.url.clone()]).await;
        let space = own_space(&keys);
        let tag = space_tag(&r.engine, &space).await;
        // ~60k characters of change base64: sealed into two `c` parts.
        let extra = [
            ("channel", json!({"stringValue": "space"})),
            ("body", json!({"stringValue": "x".repeat(45_000)})),
        ];
        let parts = seal(
            &r.engine,
            &keys,
            note(&keys, "big", &extra),
            3000,
            Some(&space),
        )
        .await;
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|e| e.tag("c").is_some()));
        relay.insert(parts[0].clone());
        r.sync.set_shared_spaces(vec![space]).await.unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        // An open group is unfinished history.
        assert_eq!(r.host.passes.lock().last(), Some(&(false, true)));
        assert!(held(&*r.store, &tag).await.is_empty());

        parts[1..].iter().for_each(|e| relay.insert(e.clone()));
        let before = relay.world().id_fetches.len();
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        let again: HashSet<String> = fetched(&relay, before).into_iter().collect();
        assert_eq!(again, ids(&parts));
        let held_parts: HashSet<String> = held(&*r.store, &tag).await.into_iter().collect();
        assert_eq!(held_parts, ids(&parts));
        assert_eq!(changes(&*r.store, "big").await.len(), 1);

        let before = relay.world().id_fetches.len();
        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        assert!(fetched(&relay, before).is_empty());
        r.sync.stop();
    });
}

/// A relay that refuses NEG-OPEN is walked page by page, and not asked
/// again this session.
fn a_refused_neg_open_falls_back_to_the_paged_walk(refuse: impl FnOnce(&TestRelay, &RelayProxy)) {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let proxy = RelayProxy::start(&relay.url);
    refuse(&relay, &proxy);
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![proxy.url.clone()]).await;
        for i in 0..2u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            relay.insert(single(&r.engine, &keys, change, 1000 + i).await);
        }
        r.sync
            .set_shared_spaces(vec![own_space(&keys)])
            .await
            .unwrap();
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert_eq!(r.host.passes.lock().last(), Some(&(true, true)));
        // Remembered after the first refusal.
        assert_eq!(proxy.rules().neg_opens, 1);
        assert!(relay.world().id_fetches.is_empty());
        let walks = dag_walks(&relay);
        assert_eq!(
            phases(walks.iter().map(kinds_shape).collect()),
            STREAM_PHASES
        );
        assert!(
            walks
                .iter()
                .all(|f| f.since == Some(1) && f.limit == Some(PAGE_LIMIT))
        );
        assert_eq!(changes(&*r.store, "doc1").await.len(), 1);
        // Walked events are held too: a later NIP-77 relay would not resend them.
        assert_eq!(held(&*r.store, "").await.len(), 2);

        assert_eq!(another_pass(&r.sync, &r.host).await, (true, true));
        assert_eq!(proxy.rules().neg_opens, 1);
        r.sync.stop();
    });
}

#[test]
fn neg_err_on_neg_open_falls_back_to_the_paged_walk_and_is_not_asked_again() {
    a_refused_neg_open_falls_back_to_the_paged_walk(|_, proxy| {
        proxy.rules().neg_err = Some("blocked: not here".into());
    });
}

#[test]
fn notice_on_neg_open_falls_back_to_the_paged_walk_and_is_not_asked_again() {
    a_refused_neg_open_falls_back_to_the_paged_walk(|relay, _| {
        relay.world().no_nip77 = true;
    });
}

#[test]
fn resume_reconciles_an_event_at_or_before_the_cursor_that_live_never_carries() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let proxy = RelayProxy::start(&relay.url);
    let keys = Keys::generate();
    block_on(async {
        let r = Replica::memory(keys.clone(), vec![proxy.url.clone()]).await;
        for i in 0..2u64 {
            let change = note(&keys, &format!("doc{i}"), &[]);
            relay.insert(single(&r.engine, &keys, change, 1000 + i).await);
        }
        r.sync.start().await.unwrap();
        r.sync.await_first_walk().await;
        assert!(r.store.bootstrapped().await.unwrap());
        // Another device comes back online and publishes an edit it made earlier.
        let late = single(&r.engine, &keys, note(&keys, "late", &[]), 999).await;
        assert!((late.created_at as i64) < r.sync.cursor() + 1);
        let subscribed = eventually(WAIT, async || {
            proxy.rules().reqs.iter().any(|filters| filters.len() > 1)
        })
        .await;
        assert!(subscribed, "live subscription up");
        let live: Vec<Filter> = proxy
            .rules()
            .reqs
            .iter()
            .filter(|filters| filters.len() > 1)
            .flatten()
            .cloned()
            .collect();
        assert!(!live.iter().any(|f| f.matches(&late)));
        relay.insert(late);

        r.host.statuses.lock().clear();
        let before = r.host.passes.lock().len();
        // Concurrent calls share one pass.
        r.sync.resume();
        r.sync.resume();
        let caught_up = eventually(WAIT, async || {
            r.host.passes.lock().len() > before
                && r.host.statuses.lock().last().map(|s| s.phase) == Some(Phase::Live)
        })
        .await;
        assert!(caught_up);
        r.sync.await_idle().await;
        assert_eq!(r.host.passes.lock().len(), before + 1);
        assert_eq!(changes(&*r.store, "late").await.len(), 1);
        let statuses = r.host.statuses.lock().clone();
        assert_eq!(statuses[0].phase, Phase::Backfill);
        assert_eq!(statuses[0].detail.as_deref(), Some("catching up"));
        assert_eq!(statuses.last().unwrap().phase, Phase::Live);
        r.sync.stop();
    });
}
