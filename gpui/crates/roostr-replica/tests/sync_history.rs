//! History completion and recovery: when a pass counts as complete, how a
//! partial chunk group holds the cursor below its parts until a covering
//! repair (by scan or live), import faults and races with live events, the
//! obsolete checkpoint floor, a newly joined space, and pages persisting as
//! they land. Disk faults and races are injected through [`HookedStore`].
#![cfg(not(target_family = "wasm"))]

#[path = "support/hooked_store.rs"]
mod hooked_store;
#[path = "support/space_fixture.rs"]
mod space_fixture;
mod support;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures::StreamExt;
use futures::channel::oneshot;
use hooked_store::HookedStore;
use roostr_engine::{Change, Engine, SealRequest};
use roostr_nostr::spacekeys::{InMemorySpaceKeyring, KeyFile};
use roostr_nostr::{Event, EventTemplate, Filter, Keys, kinds};
use roostr_replica::local::{InMemoryLocalKv, LocalKv};
use roostr_replica::sync::{Keyring, PAGE_LIMIT, Phase, RelaySync, SyncHost};
use roostr_replica_store::{ChangeStore, HeldQuery, SqliteChangeStore, StoreError};
use serde_json::{Value, json};
use space_fixture::{SPACE_ID, World};
use support::relay::TestRelay;
use support::{ENGINE_TURN, TestHost, block_on, eventually};

const WAIT: Duration = Duration::from_secs(30);

/// A replica's sync over a [`HookedStore`].
struct Device {
    sync: RelaySync,
    host: Arc<TestHost>,
    store: Arc<HookedStore>,
}

impl Device {
    fn new(keys: &Keys, relays: Vec<String>, store: Arc<HookedStore>) -> Device {
        let host = Arc::new(TestHost::default());
        let sync = RelaySync::new(
            Engine::native().expect("native engine"),
            store.clone() as Arc<dyn ChangeStore>,
            keys.clone(),
            relays,
            Arc::new(InMemorySpaceKeyring::new(KeyFile::default())) as Arc<dyn Keyring>,
            Arc::new(InMemoryLocalKv::new()) as Arc<dyn LocalKv>,
            host.clone() as Arc<dyn SyncHost>,
        );
        Device { sync, host, store }
    }

    async fn start(&self, w: &World) {
        self.sync.set_shared_spaces(vec![w.space()]).await.unwrap();
        self.sync.start().await.unwrap();
        self.sync.await_first_walk().await;
    }

    fn last_pass(&self) -> Option<(bool, bool)> {
        self.host.passes.lock().last().copied()
    }

    /// Runs one more history pass and waits for it to end.
    async fn another_pass(&self) {
        let before = self.host.passes.lock().len();
        self.sync.resume();
        let done = eventually(WAIT, async || self.host.passes.lock().len() > before).await;
        assert!(done, "the catch-up pass ended");
    }

    async fn cursor(&self) -> i64 {
        self.store.cursor().await.unwrap()
    }

    async fn replay_groups(&self) -> usize {
        self.store.replay_groups().await.unwrap().len()
    }

    async fn changes(&self, object_id: &str) -> usize {
        self.store
            .changes_for(object_id.into())
            .await
            .unwrap()
            .len()
    }
}

async fn file_store(path: &std::path::Path) -> Arc<HookedStore> {
    Arc::new(HookedStore::new(
        SqliteChangeStore::open(path).await.unwrap(),
    ))
}

async fn memory_store() -> Arc<HookedStore> {
    Arc::new(HookedStore::new(
        SqliteChangeStore::in_memory().await.unwrap(),
    ))
}

/// A relay URL nothing listens on.
fn dead_relay() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("ws://{}", listener.local_addr().unwrap())
}

/// Open chunk groups in the (process-global) receive session.
async fn open_groups(engine: &Engine) -> i64 {
    engine.session_state().await.unwrap().groups
}

/// `doc`: a note scoped to the space, by the owner.
async fn create_doc(w: &World) -> Change {
    w.change(
        "doc",
        json!([
            {"objectCreate": {"typeKey": "note"}},
            {"fieldSet": {"key": "channel", "value": {"stringValue": SPACE_ID}}},
        ]),
        &[],
        None,
    )
    .await
}

/// A note named `object_id`, scoped to the space.
async fn scoped(w: &World, object_id: &str) -> Change {
    w.change(
        object_id,
        json!([
            {"objectCreate": {"typeKey": "note"}},
            {"fieldSet": {"key": "channel", "value": {"stringValue": SPACE_ID}}},
            {"fieldSet": {"key": "name", "value": {"stringValue": object_id}}},
        ]),
        &[],
        None,
    )
    .await
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn kinds_of(filter: &Filter) -> String {
    let kinds: Vec<String> = filter.kinds.iter().flatten().map(u32::to_string).collect();
    kinds.join(",")
}

/// The history walks a relay saw (one-filter REQs other than gift wraps).
fn history_walks(relay: &TestRelay) -> Vec<Filter> {
    relay
        .world()
        .walks
        .iter()
        .filter(|f| {
            !f.kinds
                .as_ref()
                .is_some_and(|k| k.contains(&kinds::GIFT_WRAP))
        })
        .cloned()
        .collect()
}

fn store_error(message: &str) -> StoreError {
    StoreError::Backend(message.into())
}

// ── A relay that ends every REQ with CLOSED ─────────────────────────

/// Accepts sockets, refuses NEG-OPEN like a relay without NIP-77 and ends
/// every `REQ` with `CLOSED "unavailable"` instead of an EOSE.
fn closing_relay() -> String {
    let listener = block_on(async_net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        block_on(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                std::thread::spawn(move || block_on(serve_closing(stream)));
            }
        })
    });
    url
}

async fn serve_closing(stream: async_net::TcpStream) {
    let Ok(mut socket) = async_tungstenite::accept_async(stream).await else {
        return;
    };
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Vec<Value> = serde_json::from_str(text.as_str()).unwrap();
        let id = frame.get(1).cloned().unwrap_or(Value::Null);
        let reply = match frame[0].as_str() {
            Some("REQ") => json!(["CLOSED", id, "unavailable"]),
            Some("NEG-OPEN") => json!(["NOTICE", "ERROR: bad msg: unknown cmd"]),
            _ => continue,
        };
        if socket.send(Message::text(reply.to_string())).await.is_err() {
            return;
        }
    }
}

#[test]
fn an_empty_real_eose_completes_history_but_a_closed_subscription_does_not() {
    let _turn = ENGINE_TURN.lock();
    let empty = TestRelay::start();
    empty.world().no_nip77 = true;
    let closing = closing_relay();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let a = Device::new(&w.owner, vec![empty.url.clone()], memory_store().await);
        a.start(&w).await;
        assert!(!history_walks(&empty).is_empty(), "paged by REQ");
        assert_eq!(
            a.last_pass(),
            Some((true, true)),
            "empty EOSEs are a complete answer"
        );
        a.sync.stop();

        let b = Device::new(&w.owner, vec![closing], memory_store().await);
        b.start(&w).await;
        assert_eq!(
            b.last_pass(),
            Some((false, true)),
            "CLOSED is a failure, never a short page"
        );
        b.sync.stop();
    });
}

#[test]
fn a_complete_chunk_repair_releases_the_durable_cursor_floor() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let parts = w.chunk_events(&doc, &w.owner, 100).await;
        relay.insert(parts[0].clone());
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        // The reconcile covers the whole stream, older than the cursor too.
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((false, true)));
        assert_eq!(d.cursor().await, 99, "held below the open group's part");
        assert_eq!(d.replay_groups().await, 1);

        // The repair walks pages (a relay without NIP-77) from event zero.
        relay.world().no_nip77 = true;
        relay.insert(parts[1].clone());
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 200, "the floor is released");
        assert_eq!(d.replay_groups().await, 0);
        assert_eq!(d.changes("doc").await, 1);
        let walks = history_walks(&relay);
        assert!(!walks.is_empty());
        assert!(
            walks
                .iter()
                .all(|f| f.limit == Some(PAGE_LIMIT) && f.since == Some(0)),
            "{walks:?}"
        );
        d.sync.stop();
    });
}

#[test]
fn an_ordinary_live_chunk_success_restores_the_cursor_without_another_scan() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((true, true)));

        // Live parts are newer than the cursor (the subscription starts after it).
        let parts = w.chunk_events(&doc, &w.owner, 300).await;
        relay.insert(parts[0].clone());
        let held = eventually(WAIT, async || d.cursor().await == 299).await;
        assert!(held, "the open group holds the cursor below its part");
        relay.insert(parts[1].clone());
        let restored = eventually(WAIT, async || d.cursor().await == 300).await;
        assert!(restored, "assembling the group restores the cursor");
        assert_eq!(d.changes("doc").await, 1);
        assert_eq!(d.host.passes.lock().len(), 1, "no further history scan");
        d.sync.stop();
    });
}

#[test]
fn failed_scans_retain_discarded_groups_across_reload_until_a_covering_repair() {
    let _turn = ENGINE_TURN.lock();
    let first = TestRelay::start();
    first.world().no_nip77 = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let parts = w.chunk_events(&doc, &w.owner, 100).await;
        first.insert(parts[0].clone());

        // One relay delivers the first part, the other is unreachable.
        let store = file_store(&path).await;
        store.set_cursor(200, None).await.unwrap();
        let a = Device::new(&w.owner, vec![first.url.clone(), dead_relay()], store);
        a.start(&w).await;
        assert_eq!(a.last_pass().map(|p| p.0), Some(false));
        assert_eq!(open_groups(&Engine::native().unwrap()).await, 1);
        let first_walks = history_walks(&first).len();
        a.sync.stop();
        a.store.close().await.unwrap();
        drop(a);

        let reopened = SqliteChangeStore::open(&path).await.unwrap();
        assert_eq!(
            reopened.cursor().await.unwrap(),
            0,
            "the failed scan faulted the floor"
        );
        assert_eq!(reopened.replay_groups().await.unwrap().len(), 1);
        reopened.close().await.unwrap();

        // Restarted against a relay holding nothing: still owed.
        let repair = TestRelay::start();
        repair.world().no_nip77 = true;
        let b = Device::new(&w.owner, vec![repair.url.clone()], file_store(&path).await);
        b.start(&w).await;
        assert_eq!(b.last_pass(), Some((false, true)));
        assert_eq!(b.cursor().await, 0);
        assert_eq!(b.replay_groups().await, 1);

        // A covering repair: both parts, walked from event zero.
        for part in &parts {
            repair.insert(part.clone());
        }
        b.another_pass().await;
        assert_eq!(b.last_pass(), Some((true, true)));
        let walks = history_walks(&repair);
        assert!(walks.iter().all(|f| f.since == Some(0)), "{walks:?}");
        assert_eq!(b.cursor().await, 100);
        assert_eq!(b.replay_groups().await, 0);
        assert_eq!(b.changes("doc").await, 1);
        assert_eq!(
            history_walks(&first).len(),
            first_walks,
            "the stopped walker never ran again"
        );
        b.sync.stop();
    });
}

#[test]
fn an_import_failure_survives_reload_and_a_later_clean_scan_retires_it() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        for part in w.chunk_events(&doc, &w.owner, 100).await {
            relay.insert(part);
        }
        let store = file_store(&path).await;
        store.set_cursor(200, None).await.unwrap();
        let failed = Arc::new(AtomicBool::new(false));
        let once = failed.clone();
        store.on_import(move |_| {
            let fail = !once.swap(true, Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(store_error("disk full"))
                } else {
                    Ok(())
                }
            })
        });
        let d = Device::new(&w.owner, vec![relay.url.clone()], store);
        d.start(&w).await;
        assert!(failed.load(Ordering::SeqCst));
        assert!(
            d.host.passes.lock().is_empty(),
            "the pass failed, it did not complete"
        );
        let error = d
            .host
            .statuses
            .lock()
            .iter()
            .rev()
            .find(|s| s.phase == Phase::Error)
            .and_then(|s| s.detail.clone())
            .unwrap_or_default();
        assert!(error.contains("disk full"), "{error}");

        let reopened = SqliteChangeStore::open(&path).await.unwrap();
        assert_eq!(reopened.cursor().await.unwrap(), 0);
        assert_eq!(reopened.replay_groups().await.unwrap().len(), 1);
        reopened.close().await.unwrap();

        d.store.clear_hooks();
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 200);
        assert_eq!(d.replay_groups().await, 0);
        assert_eq!(open_groups(&Engine::native().unwrap()).await, 0);
        assert_eq!(d.changes("doc").await, 1);
        d.sync.stop();
    });
}

#[test]
fn a_fresh_conflict_in_an_otherwise_complete_scan_needs_another_clean_covering_pass() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let (parts, gid) = w.chunks(&doc).await;
        let part = |i: usize, total: &str| {
            w.event(
                kinds::CHANGE,
                &parts[i],
                &w.owner,
                &World::c_tag(&gid, &i.to_string(), total),
                None,
                100,
            )
        };
        // The relay holds a part whose total disagrees with its sibling's.
        let conflicting = [part(0, "2"), part(1, "3")];
        for event in &conflicting {
            relay.insert(event.clone());
        }
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((false, true)));
        assert_eq!(open_groups(&Engine::native().unwrap()).await, 0);
        assert_eq!(d.cursor().await, 99);

        // The conflicting events are gone; a pass finding nothing repairs nothing.
        relay.world().events.clear();
        relay.world().no_nip77 = true;
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((false, true)));

        let repair_from = relay.world().walks.len();
        relay.insert(part(0, "2"));
        relay.insert(part(1, "2"));
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        let changes: Vec<Filter> = relay.world().walks[repair_from..]
            .iter()
            .filter(|f| f.kinds.as_deref() == Some(&[kinds::CHANGE]))
            .cloned()
            .collect();
        assert_eq!(changes.len(), 2, "the personal and the space stream");
        assert!(changes.iter().all(|f| f.since == Some(0)), "{changes:?}");
        assert_eq!(d.cursor().await, 200);
        assert_eq!(d.changes("doc").await, 1);
        d.sync.stop();
    });
}

/// Arms `store` so the next page import reports entry, waits for the
/// release, then fails (`Err`) or commits (`Ok`).
fn gate_next_write(store: &HookedStore) -> (oneshot::Receiver<()>, oneshot::Sender<bool>) {
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<bool>();
    let mut gate = Some((entered_tx, release_rx));
    store.on_import(move |_| match gate.take() {
        Some((entered, release)) => Box::pin(async move {
            let _ = entered.send(());
            match release.await {
                Ok(true) => Ok(()),
                _ => Err(store_error("disk full")),
            }
        }),
        None => Box::pin(async { Ok(()) }),
    });
    (entered_rx, release_tx)
}

#[test]
fn a_concurrent_live_import_failure_cannot_be_cleared_by_the_scan_awaiting_it() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((true, true)));

        let (entered, release) = gate_next_write(&d.store);
        relay.insert(w.change_event(&doc, &w.owner, 300).await);
        entered.await.unwrap();
        // A scan starts while the live write is mid-commit.
        let (queried_tx, queried_rx) = oneshot::channel::<()>();
        let mut queried = Some(queried_tx);
        d.store.on_held_events(move |_| {
            if let Some(tx) = queried.take() {
                let _ = tx.send(());
            }
        });
        let before = d.host.passes.lock().len();
        d.sync.resume();
        queried_rx.await.unwrap();
        release.send(false).unwrap();
        let ended = eventually(WAIT, async || d.host.passes.lock().len() > before).await;
        assert!(ended);
        assert_eq!(
            d.last_pass(),
            Some((false, true)),
            "the live fault fails the scan"
        );
        assert_eq!(d.cursor().await, 0);

        d.store.clear_hooks();
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 300);
        assert_eq!(d.changes("doc").await, 1);
        d.sync.stop();
    });
}

#[test]
fn a_live_import_still_landing_when_the_walk_ends_is_awaited_not_a_fault() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.start(&w).await;

        let (entered, release) = gate_next_write(&d.store);
        relay.insert(w.change_event(&doc, &w.owner, 300).await);
        entered.await.unwrap();
        // Release the commit once every stream filter of the scan was asked
        // for (2 scopes × bases, checkpoints, changes).
        let asked = Arc::new(AtomicUsize::new(0));
        let mut release = Some(release);
        let counter = asked.clone();
        d.store.on_held_events(move |_| {
            if counter.fetch_add(1, Ordering::SeqCst) + 1 == 6
                && let Some(release) = release.take()
            {
                let _ = release.send(true);
            }
        });
        d.another_pass().await;
        assert_eq!(asked.load(Ordering::SeqCst), 6);
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 300);
        assert_eq!(d.changes("doc").await, 1);
        d.sync.stop();
    });
}

/// A personal 1078 of `change` at `created_at`, sealed as the outbox seals.
async fn personal_event(w: &World, change: &Change, created_at: u64) -> Event {
    let bytes = w.bytes(change).await;
    let sealed = w
        .engine
        .seal(&SealRequest {
            change: &bytes,
            conversation_key: &hex::encode(w.personal_key()),
            secret: Some(&w.owner.secret_hex()),
            space: None,
            object_id: &change.object_id,
            base: None,
            nonce: None,
        })
        .await
        .unwrap();
    let part = sealed.parts.into_iter().next().unwrap();
    w.owner.sign(EventTemplate {
        kind: kinds::CHANGE,
        created_at,
        tags: part.tags,
        content: part.content,
    })
}

async fn page(w: &World, object_id: &str) -> Change {
    w.change(
        object_id,
        json!([
            {"objectCreate": {"typeKey": "page"}},
            {"fieldSet": {"key": "name", "value": {"stringValue": object_id}}},
        ]),
        &[],
        None,
    )
    .await
}

#[test]
fn a_device_that_bootstrapped_from_a_manifest_floor_walks_everything_again_and_forgets_it() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    relay.world().no_nip77 = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        // What a pre-cache-contract build left: kind 1078 walked from 500, bootstrapped.
        let store = file_store(&path).await;
        let floors = json!({"": 500, w.space_tag.clone(): 0});
        store
            .set_meta("checkpoint-floors".into(), Some(floors.to_string()))
            .await
            .unwrap();
        store.set_bootstrapped().await.unwrap();
        store.set_cursor(700, None).await.unwrap();
        let d = Device::new(&w.owner, vec![relay.url.clone()], store);
        d.start(&w).await;

        // Both scopes from event zero: bases, then checkpoints (the space's
        // with its owner's kind-5 h-deletion), then changes.
        let walks = history_walks(&relay);
        assert!(walks.iter().all(|f| f.since == Some(1)), "{walks:?}");
        let order: Vec<String> = walks.iter().map(kinds_of).collect();
        assert_eq!(order.len(), 6, "{order:?}");
        let phase = |range: std::ops::Range<usize>| {
            let mut kinds = order[range].to_vec();
            kinds.sort();
            kinds
        };
        assert_eq!(phase(0..2), ["31078", "31078"]);
        assert_eq!(phase(2..4), ["1079", "1079,5"]);
        assert_eq!(phase(4..6), ["1078", "1078"]);
        assert!(
            d.store.bootstrapped().await.unwrap(),
            "the clean walk re-earned it"
        );
        assert_eq!(
            d.store.meta("checkpoint-floors".into()).await.unwrap(),
            None
        );

        // Live stays incremental: from the cursor (700), not from zero.
        let older = page(&w, "older").await;
        let newer = page(&w, "newer").await;
        relay.insert(personal_event(&w, &older, 650).await);
        relay.insert(personal_event(&w, &newer, 750).await);
        let heard = eventually(WAIT, async || d.changes("newer").await == 1).await;
        assert!(heard, "a live event after the cursor arrives");
        assert_eq!(d.changes("older").await, 0, "live starts after the cursor");
        d.sync.stop();
        d.store.close().await.unwrap();
        drop(d);

        // Starting again is an ordinary incremental walk: the floor record is gone.
        let seen = relay.world().walks.len();
        let again = Device::new(&w.owner, vec![relay.url.clone()], file_store(&path).await);
        again.start(&w).await;
        let later: Vec<Filter> = relay.world().walks[seen..]
            .iter()
            .filter(|f| {
                f.kinds
                    .as_ref()
                    .is_some_and(|k| !k.contains(&kinds::GIFT_WRAP))
            })
            .cloned()
            .collect();
        assert_eq!(later.len(), 6);
        assert!(later.iter().all(|f| f.since == Some(751)), "{later:?}");
        again.sync.stop();
    });
}

#[test]
fn a_newly_joined_space_is_walked_from_event_zero_even_when_set_again_at_once() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    relay.world().no_nip77 = true;
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.sync.start().await.unwrap();
        d.sync.await_first_walk().await;
        let seen = relay.world().walks.len();

        // The space list re-renders twice in one tick after a join.
        d.sync.set_shared_spaces(vec![w.space()]).await.unwrap();
        d.sync.set_shared_spaces(vec![w.space()]).await.unwrap();
        let walked = eventually(WAIT, async || d.host.passes.lock().len() == 2).await;
        assert!(walked, "the join ran one pass");
        roostr_nostr::sleep(Duration::from_millis(300)).await;
        assert_eq!(d.host.passes.lock().len(), 2, "and only one");
        let space_walks: Vec<Filter> = relay.world().walks[seen..]
            .iter()
            .filter(|f| f.tags.contains_key(&'h'))
            .cloned()
            .collect();
        let order: Vec<String> = space_walks.iter().map(kinds_of).collect();
        assert_eq!(order, ["31078", "1079,5", "1078"]);
        assert!(
            space_walks.iter().all(|f| f.since == Some(1)),
            "{space_walks:?}"
        );
        d.sync.stop();
    });
}

#[test]
fn a_suffix_first_repair_and_a_later_duplicate_suffix_leave_no_partial_group() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    relay.world().no_nip77 = true;
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let parts = w.chunk_events(&doc, &w.owner, 100).await;
        relay.insert(parts[1].clone());
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.start(&w).await;
        assert_eq!(
            d.last_pass(),
            Some((false, true)),
            "the suffix opened a group"
        );

        let repair_from = relay.world().walks.len();
        relay.insert(parts[0].clone());
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 200);
        let engine = Engine::native().unwrap();
        assert_eq!(open_groups(&engine).await, 0);
        assert_eq!(d.replay_groups().await, 0);
        let repair: Vec<Filter> = relay.world().walks[repair_from..]
            .iter()
            .filter(|f| {
                f.kinds
                    .as_ref()
                    .is_some_and(|k| !k.contains(&kinds::GIFT_WRAP))
            })
            .cloned()
            .collect();
        assert!(repair.iter().all(|f| f.since == Some(0)), "{repair:?}");

        // The same suffix again: its group is imported, never reopened.
        let again = engine
            .ingest(&space_fixture::relay_event(&parts[1]), now_ms())
            .await
            .unwrap();
        assert!(again.item.is_none());
        let state = engine.session_state().await.unwrap();
        assert_eq!(state.groups, 0);
        assert!(state.replay_groups.is_empty());
        assert_eq!(d.cursor().await, 200);
        d.sync.stop();
    });
}

#[test]
fn a_chunk_arriving_during_the_repair_cursor_commit_keeps_recovery_pending() {
    let _turn = ENGINE_TURN.lock();
    let relay = Arc::new(TestRelay::start());
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let doc = create_doc(&w).await;
        let parts = w.chunk_events(&doc, &w.owner, 300).await;
        let d = Device::new(&w.owner, vec![relay.url.clone()], memory_store().await);
        d.store.set_cursor(200, None).await.unwrap();
        d.store.set_bootstrapped().await.unwrap();
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((true, true)));

        // A pass persists the cursor as it starts and again as it completes:
        // the first part arrives (live) while the completing commit runs.
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let (relay_ref, part) = (relay.clone(), parts[0].clone());
        d.store.on_set_cursor(move |_| {
            if counter.fetch_add(1, Ordering::SeqCst) + 1 != 2 {
                return Box::pin(async {});
            }
            relay_ref.insert(part.clone());
            Box::pin(async {
                let engine = Engine::native().unwrap();
                let deadline = std::time::Instant::now() + WAIT;
                while engine.session_state().await.unwrap().groups != 1 {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the part arrived during the commit"
                    );
                    roostr_nostr::sleep(Duration::from_millis(20)).await;
                }
            })
        });
        d.another_pass().await;
        assert!(calls.load(Ordering::SeqCst) >= 2);
        assert_eq!(d.last_pass(), Some((false, true)), "recovery stays pending");
        assert_eq!(open_groups(&Engine::native().unwrap()).await, 1);
        assert_eq!(d.replay_groups().await, 1);
        assert!(
            d.cursor().await <= 200,
            "the cursor did not pass the open group"
        );

        d.store.clear_hooks();
        // On the relay but not pushed live: only a scan can repair it.
        relay.world().events.push(parts[1].clone());
        d.another_pass().await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(d.cursor().await, 300);
        assert_eq!(d.changes("doc").await, 1);
        d.sync.stop();
    });
}

#[test]
fn a_full_page_is_in_the_store_before_the_walk_asks_for_the_next() {
    let _turn = ENGINE_TURN.lock();
    let relay = Arc::new(TestRelay::start());
    relay.world().no_nip77 = true;
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        for i in 0..PAGE_LIMIT {
            let object = if i == PAGE_LIMIT - 1 {
                "space-note".to_string()
            } else {
                format!("note-{i}")
            };
            let note = scoped(&w, &object).await;
            relay.insert(w.change_event(&note, &w.owner, 1_000 + i).await);
        }
        let store = memory_store().await;
        // When the newest event's change is written, has the walk asked for page two yet?
        let paged_at_write = Arc::new(parking_lot::Mutex::new(None));
        let (seen, relay_ref) = (paged_at_write.clone(), relay.clone());
        store.on_import(move |records| {
            if records.iter().any(|r| r.object_id == "space-note") {
                *seen.lock() = Some(relay_ref.world().paged_reqs);
            }
            Box::pin(async { Ok(()) })
        });
        // A third device of the space: only the space stream holds the page.
        let d = Device::new(&space_fixture::keys(5), vec![relay.url.clone()], store);
        d.start(&w).await;
        assert_eq!(
            *paged_at_write.lock(),
            Some(0),
            "stored before the next page"
        );
        assert!(
            relay.world().paged_reqs >= 1,
            "a full page kept the walk going"
        );
        assert_eq!(d.changes("space-note").await, 1);
        d.sync.stop();
    });
}

#[test]
fn per_relay_paging_loses_no_change_held_by_only_one_relay() {
    let _turn = ENGINE_TURN.lock();
    let (a, b) = (TestRelay::start(), TestRelay::start());
    a.world().no_nip77 = true;
    b.world().no_nip77 = true;
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let base = 1_000u64;
        let mut ids = Vec::new();
        // Interleaved histories, each over a page deep.
        for i in 0..300u64 {
            let change = page(&w, &format!("note-{i:03}")).await;
            ids.push(change.id.clone());
            let relay = if i % 2 == 0 { &a } else { &b };
            relay.insert(personal_event(&w, &change, base + i).await);
        }
        // One object's create on one relay, its delete (newer) on the other only.
        let create = page(&w, "gap").await;
        let delete = w
            .change("gap", json!([{"objectDelete": {}}]), &[&create.id], None)
            .await;
        a.insert(personal_event(&w, &create, base + 3).await);
        b.insert(personal_event(&w, &delete, base + 150).await);
        ids.extend([create.id.clone(), delete.id.clone()]);

        let d = Device::new(
            &w.owner,
            vec![a.url.clone(), b.url.clone()],
            memory_store().await,
        );
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert!(a.world().paged_reqs >= 1 && b.world().paged_reqs >= 1);
        let gap: Vec<String> = d
            .store
            .changes_for("gap".into())
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(gap.len(), 2, "both the create and the delete landed");
        assert!(gap.contains(&create.id) && gap.contains(&delete.id));
        let mut held = 0;
        for object in d.store.object_ids().await.unwrap() {
            held += d.changes(&object).await;
        }
        assert_eq!(held, ids.len());
        d.sync.stop();
    });
}

/// Every change id the store holds, counted once per stored row.
async fn stored_change_ids(store: &HookedStore) -> Vec<String> {
    let mut ids = Vec::new();
    for object in store.object_ids().await.unwrap() {
        ids.extend(
            store
                .changes_for(object)
                .await
                .unwrap()
                .into_iter()
                .map(|c| c.id),
        );
    }
    ids
}

/// The personal stream's held change events.
async fn held_personal(store: &HookedStore) -> HashSet<String> {
    let query = HeldQuery {
        kinds: vec![kinds::CHANGE],
        ..HeldQuery::default()
    };
    store
        .held_events(String::new(), query)
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.id)
        .collect()
}

#[test]
fn a_page_lands_in_one_write_and_a_reload_mid_walk_loses_and_duplicates_nothing() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    relay.world().no_nip77 = true;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replica.sqlite");
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        let total = 2 * PAGE_LIMIT + 40;
        let (mut change_ids, mut event_ids) = (HashSet::new(), HashSet::new());
        for i in 0..total {
            let change = page(&w, &format!("note-{i:03}")).await;
            let event = personal_event(&w, &change, 1_000 + i).await;
            change_ids.insert(change.id);
            event_ids.insert(event.id.clone());
            relay.insert(event);
        }
        let store = file_store(&path).await;
        // The first page commits; the tab goes away while the second is written.
        let writes = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = writes.clone();
        store.on_import(move |records| {
            let mut writes = seen.lock();
            writes.push(records.len());
            let cut = writes.len() > 1;
            Box::pin(async move {
                if cut {
                    Err(store_error("the tab went away"))
                } else {
                    Ok(())
                }
            })
        });
        let d = Device::new(&w.owner, vec![relay.url.clone()], store);
        d.start(&w).await;
        d.sync.stop();
        // One write per page, the whole page in it (the second page repeats
        // the first's oldest second, seen already).
        let pages = writes.lock().clone();
        assert_eq!(pages.len(), 2, "{pages:?}");
        assert_eq!(pages[0], PAGE_LIMIT as usize);
        assert!(pages[1] >= PAGE_LIMIT as usize - 1, "{pages:?}");
        // All of the first page, nothing of the second: no change, no held
        // event (which would keep a reconcile from asking for it again), no
        // publication mark.
        let first = stored_change_ids(&d.store).await;
        assert_eq!(first.len(), PAGE_LIMIT as usize);
        let held = held_personal(&d.store).await;
        assert_eq!(held.len(), PAGE_LIMIT as usize);
        assert!(held.is_subset(&event_ids));
        let mut marked = 0;
        for id in &change_ids {
            marked += usize::from(d.store.is_published(id.clone()).await.unwrap());
        }
        assert_eq!(marked, PAGE_LIMIT as usize);
        d.store.close().await.unwrap();

        // The reload: a fresh sync over the same database finishes the walk.
        let e = Device::new(&w.owner, vec![relay.url.clone()], file_store(&path).await);
        e.start(&w).await;
        assert_eq!(e.last_pass(), Some((true, true)));
        let stored = stored_change_ids(&e.store).await;
        assert_eq!(stored.len(), total as usize, "every change exactly once");
        assert_eq!(stored.into_iter().collect::<HashSet<_>>(), change_ids);
        assert_eq!(held_personal(&e.store).await, event_ids);
        for id in &change_ids {
            assert!(e.store.is_published(id.clone()).await.unwrap(), "{id}");
        }
        assert_eq!(
            e.sync.stats().imported,
            total - PAGE_LIMIT,
            "what the first session stored is not imported again"
        );
        e.sync.stop();
    });
}

#[test]
fn a_shared_page_editing_what_it_creates_is_judged_against_its_own_earlier_changes() {
    let _turn = ENGINE_TURN.lock();
    let relay = TestRelay::start();
    block_on(async {
        let w = World::new(Engine::native().unwrap()).await;
        // One page: the doc's create, then edits each naming the previous.
        let mut previous = create_doc(&w).await;
        relay.insert(w.change_event(&previous, &w.owner, 100).await);
        for i in 0..3u64 {
            let edit = w
                .change(
                    "doc",
                    json!([{"fieldSet": {"key": "name", "value": {"stringValue": format!("v{i}")}}}]),
                    &[&previous.id],
                    None,
                )
                .await;
            relay.insert(w.change_event(&edit, &w.owner, 101 + i).await);
            previous = edit;
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let seen = writes.clone();
        let store = memory_store().await;
        store.on_import(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
        // A member's device: every payload is authorized against the space.
        let d = Device::new(&space_fixture::keys(5), vec![relay.url.clone()], store);
        d.start(&w).await;
        assert_eq!(d.last_pass(), Some((true, true)));
        assert_eq!(
            d.changes("doc").await,
            4,
            "every edit authorized and stored"
        );
        // Each verdict saw its predecessors, still buffered: one write.
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        d.sync.stop();
    });
}
