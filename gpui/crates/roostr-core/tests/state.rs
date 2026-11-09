//! The store's async paths and the State's event dispatch, over a fake
//! backend whose object fetches and collection refreshes the test answers
//! by hand, on roostr-rt's virtual clock (no sleeps).
#![cfg(not(target_family = "wasm"))]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures::channel::{mpsc, oneshot};
use parking_lot::Mutex;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::cover::CoverAsk;
use roostr_core::nav::{self, NavEvent, Route};
use roostr_core::store::USER_PREFS_ID;
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, SyncPhase, SyncStatus,
};
use roostr_core::{ObjectSlot, State, StoreEvent};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use serde_json::json;

/// The store's commit throttle and reconnect wait (store.rs).
const THROTTLE: Duration = Duration::from_millis(800);
const RECONNECT: Duration = Duration::from_secs(2);

/// An object fetch: the id, and where its answer goes until it is sent.
type Fetch = (String, Option<oneshot::Sender<Result<Object>>>);

#[derive(Default)]
struct Fake {
    /// Object fetches in call order, each waiting for the test's answer.
    fetches: Mutex<Vec<Fetch>>,
    /// Collection refreshes started (`objects` calls).
    refreshes: AtomicUsize,
    /// Hold refreshes until [`Fake::release_refreshes`].
    hold_refreshes: AtomicBool,
    held_refreshes: Mutex<Vec<oneshot::Sender<()>>>,
    /// One sender per commit-stream connection.
    streams: Mutex<Vec<mpsc::UnboundedSender<Result<BackendEvent>>>>,
    mutations: AtomicUsize,
}

impl Fake {
    fn fetches_of(&self, id: &str) -> usize {
        self.fetches.lock().iter().filter(|(i, _)| i == id).count()
    }

    /// Answer the oldest unanswered fetch of `id` with an object whose `rev` is `rev`.
    fn answer(&self, id: &str, rev: i64) {
        let mut fetches = self.fetches.lock();
        let (_, tx) = fetches
            .iter_mut()
            .find(|(i, tx)| i == id && tx.is_some())
            .unwrap_or_else(|| panic!("no fetch of {id} waiting"));
        let object = Object {
            id: id.to_string(),
            updated_at: rev,
            ..Object::default()
        };
        let _ = tx.take().expect("unanswered").send(Ok(object));
    }

    fn release_refreshes(&self) {
        for tx in self.held_refreshes.lock().drain(..) {
            let _ = tx.send(());
        }
    }

    fn send(&self, event: Result<BackendEvent>) {
        self.streams
            .lock()
            .last()
            .expect("a stream is open")
            .unbounded_send(event)
            .expect("the store reads its stream");
    }

    fn commit(&self, id: &str) {
        self.send(Ok(BackendEvent::Committed(id.to_string())));
    }
}

/// The store's handle on the fake.
struct Handle(Arc<Fake>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "fake".into()
    }

    fn settings(&self) -> BoxFuture<Result<Settings>> {
        boxed(async {
            Ok(Settings {
                has_key: true,
                ..Settings::default()
            })
        })
    }

    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        self.0.refreshes.fetch_add(1, Ordering::SeqCst);
        if !self.0.hold_refreshes.load(Ordering::SeqCst) {
            return boxed(async { Ok(Vec::new()) });
        }
        let (tx, rx) = oneshot::channel();
        self.0.held_refreshes.lock().push(tx);
        boxed(async move {
            rx.await.map_err(|_| anyhow!("refresh dropped"))?;
            Ok(Vec::new())
        })
    }

    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        if id == USER_PREFS_ID {
            return boxed(async { Err(anyhow!("no preferences")) });
        }
        let (tx, rx) = oneshot::channel();
        self.0.fetches.lock().push((id.to_string(), Some(tx)));
        boxed(async move { rx.await.map_err(|_| anyhow!("fetch dropped"))? })
    }

    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        boxed(async { Ok(Vec::new()) })
    }

    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        boxed(async { Ok(Vec::new()) })
    }

    fn query(&self, _body: &serde_json::Value) -> BoxFuture<Result<QueryResult>> {
        boxed(async { Ok(QueryResult::default()) })
    }

    fn mutate(&self, _action: &str, _params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        self.0.mutations.fetch_add(1, Ordering::SeqCst);
        boxed(async { Ok(JsonMap::new()) })
    }

    fn serving(&self, _object_ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }

    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }

    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        let (tx, rx) = mpsc::unbounded();
        self.0.streams.lock().push(tx);
        boxed_stream(rx)
    }
}

/// What the listeners heard.
#[derive(Default)]
struct Heard {
    store: Vec<StoreEvent>,
    nav: Vec<NavEvent>,
    covers: Vec<CoverAsk>,
}

struct App {
    s: State,
    fake: Arc<Fake>,
    woke: Receiver<()>,
}

impl App {
    /// A connected, loaded app over a fresh fake.
    fn new() -> Self {
        static UI_STATE: Once = Once::new();
        UI_STATE.call_once(|| {
            let path = std::env::temp_dir().join(format!(
                "roostr-core-state-test-{}.json",
                std::process::id()
            ));
            // SAFETY: set once, before any test thread reads it (every test starts here).
            unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
        });
        let fake = Arc::new(Fake::default());
        let mut s = State::new(Arc::new(Handle(fake.clone())), None);
        let (tx, woke) = channel();
        let tx = Mutex::new(tx);
        s.rt.set_waker(Arc::new(move || {
            let _ = tx.lock().send(());
        }));
        s.on_store(|s, e| s.slot::<Heard>().store.push(e.clone()));
        s.on_nav(|s, e| s.slot::<Heard>().nav.push(e.clone()));
        s.on_cover_ask(|s, e| s.slot::<Heard>().covers.push(e.clone()));
        let mut app = App { s, fake, woke };
        app.until("the first refresh and the commit stream", |s, f| {
            s.store.loaded && f.streams.lock().len() == 1
        });
        app
    }

    /// Poll until `done`, waiting for the executor's wakes in between.
    fn until(&mut self, what: &str, done: impl Fn(&State, &Fake) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.s.poll();
            if done(&self.s, &self.fake) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            let _ = self.woke.recv_timeout(left);
        }
    }

    fn heard(&mut self) -> &mut Heard {
        self.s.slot::<Heard>()
    }

    fn committed(&mut self, n: usize) {
        self.until("the commits", |s, _| {
            s.get::<Heard>().map_or(0, |h| {
                h.store
                    .iter()
                    .filter(|e| matches!(e, StoreEvent::Committed(_)))
                    .count()
            }) >= n
        });
    }

    fn rev(&self, id: &str) -> Option<i64> {
        match self.s.store.object_slot(id) {
            Some(ObjectSlot::Ready(object)) => Some(object.updated_at),
            _ => None,
        }
    }
}

#[test]
fn a_commit_mid_fetch_fetches_once_more_after_it() {
    let mut app = App::new();
    app.s.store.ensure_object("a");
    assert!(matches!(
        app.s.store.object_slot("a"),
        Some(ObjectSlot::Loading)
    ));
    app.until("the first fetch", |_, f| f.fetches_of("a") == 1);
    // Two commits land while it is in flight: one more fetch, not two.
    app.fake.commit("a");
    app.fake.commit("a");
    app.committed(2);
    assert_eq!(app.fake.fetches_of("a"), 1);
    app.fake.answer("a", 1);
    app.until("the queued fetch", |_, f| f.fetches_of("a") == 2);
    assert_eq!(app.rev("a"), Some(1), "the first answer shows meanwhile");
    app.fake.answer("a", 2);
    app.until(
        "the second answer",
        |s, _| matches!(s.store.object_slot("a"), Some(ObjectSlot::Ready(o)) if o.updated_at == 2),
    );
    app.s.poll();
    assert_eq!(app.fake.fetches_of("a"), 2);
}

#[test]
fn commits_refresh_the_collections_once_per_throttle_window() {
    let mut app = App::new();
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 1);
    for id in ["x", "y", "z"] {
        app.fake.commit(id);
    }
    app.committed(3);
    // Uncached objects are not fetched; the refresh waits for the throttle.
    assert_eq!(app.fake.fetches.lock().len(), 0);
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 1);
    app.s.rt.advance(THROTTLE);
    app.until("the throttled refresh", |_, f| {
        f.refreshes.load(Ordering::SeqCst) == 2
    });
    app.until("its answer", |s, _| {
        s.get::<Heard>()
            .is_some_and(|h| h.store.last() == Some(&StoreEvent::Refreshed))
    });
    app.s.rt.advance(THROTTLE);
    app.s.poll();
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 2);
}

#[test]
fn a_commit_during_a_refresh_queues_exactly_one_more() {
    let mut app = App::new();
    app.fake.hold_refreshes.store(true, Ordering::SeqCst);
    app.fake.commit("x");
    app.committed(1);
    app.s.rt.advance(THROTTLE);
    app.until("a refresh in flight", |_, f| {
        f.refreshes.load(Ordering::SeqCst) == 2
    });
    // Two more commits while it runs: their refresh waits for it.
    app.fake.commit("y");
    app.fake.commit("z");
    app.committed(3);
    app.s.rt.advance(THROTTLE);
    app.s.poll();
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 2);
    app.fake.hold_refreshes.store(false, Ordering::SeqCst);
    app.fake.release_refreshes();
    app.until("the running refresh's answer", |s, _| {
        // The first refresh's answer came in `App::new`.
        s.get::<Heard>().is_some_and(|h| {
            h.store
                .iter()
                .filter(|e| **e == StoreEvent::Refreshed)
                .count()
                == 2
        })
    });
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 2);
    app.s.rt.advance(THROTTLE);
    app.until("the queued refresh", |_, f| {
        f.refreshes.load(Ordering::SeqCst) == 3
    });
    app.s.rt.advance(THROTTLE);
    app.s.poll();
    assert_eq!(app.fake.refreshes.load(Ordering::SeqCst), 3);
}

#[test]
fn bootstrapped_waits_for_a_refresh_that_started_after_it() {
    let mut app = App::new();
    app.fake.hold_refreshes.store(true, Ordering::SeqCst);
    app.fake.send(Ok(BackendEvent::Status(SyncStatus {
        phase: SyncPhase::Live,
        bootstrapped: true,
        ..SyncStatus::default()
    })));
    app.until("the held refresh", |_, f| {
        f.refreshes.load(Ordering::SeqCst) == 2
    });
    assert_eq!(app.s.store.status.phase, SyncPhase::Live);
    assert!(
        !app.s.store.status.bootstrapped,
        "held until the refresh lands"
    );
    app.fake.release_refreshes();
    app.until("bootstrapped", |s, _| s.store.status.bootstrapped);
}

#[test]
fn a_failed_stream_reconnects_after_its_delay() {
    let mut app = App::new();
    app.fake.send(Err(anyhow!("connection reset")));
    app.until("the error status", |s, _| {
        s.store.status.phase == SyncPhase::Error
    });
    assert_eq!(
        app.s.store.status.detail.as_deref(),
        Some("connection reset")
    );
    app.s.poll();
    assert_eq!(app.fake.streams.lock().len(), 1);
    app.s.rt.advance(RECONNECT);
    app.until("the second connection", |_, f| f.streams.lock().len() == 2);
    app.fake.send(Ok(BackendEvent::Hello));
    app.until("live again", |s, _| s.store.status.phase == SyncPhase::Live);
    assert!(app.heard().store.contains(&StoreEvent::Reconnected));
}

#[test]
fn a_mutation_refetches_the_cached_object_it_names() {
    let mut app = App::new();
    app.s.store.ensure_object("a");
    app.until("the fetch", |_, f| f.fetches_of("a") == 1);
    app.fake.answer("a", 1);
    app.until("the object", |s, _| s.store.object("a").is_some());
    let mut params = JsonMap::new();
    params.insert("object_id".into(), json!("a"));
    roostr_core::api::mutate("set_field", params, &mut app.s).detach();
    app.until("the refetch", |_, f| f.fetches_of("a") == 2);
    assert_eq!(app.fake.mutations.load(Ordering::SeqCst), 1);
    // An uncached object is not fetched.
    let mut params = JsonMap::new();
    params.insert("object_id".into(), json!("b"));
    roostr_core::api::mutate("set_field", params, &mut app.s).detach();
    app.until("the second mutation", |_, f| {
        f.mutations.load(Ordering::SeqCst) == 2
    });
    app.s.poll();
    assert_eq!(app.fake.fetches_of("b"), 0);
}

#[test]
fn navigation_and_cover_asks_reach_their_listeners_at_the_next_poll() {
    let mut app = App::new();
    nav::navigate(Route::Object("o".into()), &mut app.s);
    roostr_core::cover::ask("o", &mut app.s);
    assert!(app.heard().nav.is_empty(), "dispatched by poll, not inline");
    app.s.poll();
    assert_eq!(app.heard().nav, vec![NavEvent::Navigated]);
    assert_eq!(
        app.heard().covers,
        vec![CoverAsk {
            object_id: "o".into()
        }]
    );
    app.s.nav.back(&mut app.s.ui);
    app.s.poll();
    assert_eq!(app.s.nav.route(), &Route::Home);
    assert_eq!(app.heard().nav.len(), 2);
}
