//! Open tabs shared through the private preferences object (`open_tabs`):
//! the debounced write, another device's change landing in place, the
//! one-time merge of this device's tabs, and binned objects' tabs closing -
//! over a fake backend that keeps the preferences, on roostr-rt's virtual
//! clock (no sleeps).
#![cfg(not(target_family = "wasm"))]

use std::collections::{BTreeMap, HashSet};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::nav::{self, Navigator, OPEN_TABS_FIELD, Route, TABS_WRITE_DELAY};
use roostr_core::store::USER_PREFS_ID;
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value,
};
use roostr_core::{State, StoreEvent};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};

/// The store's commit throttle (store.rs).
const THROTTLE: Duration = Duration::from_millis(800);

/// The vault: spaces, object summaries, binned objects and the preferences.
#[derive(Default)]
struct Vault {
    spaces: Vec<&'static str>,
    summaries: Mutex<Vec<ObjectSummary>>,
    binned: Mutex<HashSet<String>>,
    prefs: Mutex<Option<Fields>>,
    /// `set_field` writes of `open_tabs`, as written.
    tab_writes: Mutex<Vec<Value>>,
    streams: Mutex<Vec<mpsc::UnboundedSender<Result<BackendEvent>>>>,
}

impl Vault {
    fn new(spaces: &[&'static str], objects: &[(&str, &str)]) -> Arc<Vault> {
        let vault = Vault {
            spaces: spaces.to_vec(),
            ..Vault::default()
        };
        *vault.summaries.lock() = objects
            .iter()
            .map(|(id, space)| ObjectSummary {
                id: (*id).into(),
                type_key: "page".into(),
                name: (*id).into(),
                channel_id: (*space).into(),
                ..ObjectSummary::default()
            })
            .collect();
        Arc::new(vault)
    }

    /// Another device stores `tabs` as the open tabs, and its commit arrives.
    fn remote_write(&self, tabs: &[(&str, &[&str])]) {
        let tabs: BTreeMap<String, Vec<String>> = tabs
            .iter()
            .map(|(space, paths)| {
                let paths = paths.iter().map(|p| p.to_string()).collect();
                ((*space).to_string(), paths)
            })
            .collect();
        self.prefs
            .lock()
            .get_or_insert_with(Fields::new)
            .insert(OPEN_TABS_FIELD.into(), nav::open_tabs_value(&tabs));
        self.commit(USER_PREFS_ID);
    }

    /// The open tabs the preferences hold.
    fn stored(&self) -> BTreeMap<String, Vec<String>> {
        self.prefs
            .lock()
            .as_ref()
            .and_then(|f| f.get(OPEN_TABS_FIELD))
            .map(nav::open_tabs_from)
            .unwrap_or_default()
    }

    fn writes(&self) -> usize {
        self.tab_writes.lock().len()
    }

    /// Move `id` to the bin (another device did): it leaves the object list.
    fn bin(&self, id: &str) {
        self.binned.lock().insert(id.to_string());
        self.summaries.lock().retain(|o| o.id != id);
        self.commit(id);
    }

    fn commit(&self, id: &str) {
        if let Some(stream) = self.streams.lock().last() {
            let _ = stream.unbounded_send(Ok(BackendEvent::Committed(id.to_string())));
        }
    }
}

struct Handle(Arc<Vault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "vault with preferences".into()
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
        let summaries = self.0.summaries.lock().clone();
        boxed(async move { Ok(summaries) })
    }

    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let found = if id == USER_PREFS_ID {
            self.0.prefs.lock().clone().map(|fields| Object {
                id: id.into(),
                type_key: "user_prefs".into(),
                fields,
                ..Object::default()
            })
        } else {
            let binned = self.0.binned.lock().contains(id);
            let listed = self.0.summaries.lock().iter().any(|o| o.id == id);
            (binned || listed).then(|| Object {
                id: id.into(),
                type_key: "page".into(),
                deleted: binned,
                ..Object::default()
            })
        };
        let id = id.to_string();
        boxed(async move { found.ok_or_else(|| anyhow!("unknown object {id}")) })
    }

    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        let spaces = self
            .0
            .spaces
            .iter()
            .map(|id| Space {
                id: (*id).into(),
                name: (*id).into(),
                ..Space::default()
            })
            .collect();
        boxed(async move { Ok(spaces) })
    }

    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        boxed(async { Ok(Vec::new()) })
    }

    fn query(&self, _body: &serde_json::Value) -> BoxFuture<Result<QueryResult>> {
        boxed(async { Ok(QueryResult::default()) })
    }

    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        let target = params.get("object_id").or_else(|| params.get("id"));
        if target.and_then(|v| v.as_str()) == Some(USER_PREFS_ID) {
            let mut prefs = self.0.prefs.lock();
            let fields = prefs.get_or_insert_with(Fields::new);
            if action == "set_field" {
                let key = params["key"].as_str().unwrap_or_default().to_string();
                let value: Value = serde_json::from_value(params["value"].clone()).unwrap();
                if key == OPEN_TABS_FIELD {
                    self.0.tab_writes.lock().push(value.clone());
                }
                fields.insert(key, value);
            }
        }
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

/// How many commits and refreshes the store reported.
#[derive(Default)]
struct Heard {
    commits: usize,
    refreshes: usize,
}

fn heard(s: &State) -> (usize, usize) {
    s.get::<Heard>()
        .map_or((0, 0), |h| (h.commits, h.refreshes))
}

struct App {
    s: State,
    vault: Arc<Vault>,
    woke: Receiver<()>,
}

impl App {
    /// The app over `vault` with this device's saved strips `saved` (the
    /// UI state's `roostr.tabs.spaces`), loaded, in `space`.
    fn new(vault: Arc<Vault>, saved: serde_json::Value, space: &str) -> Self {
        static UI_STATE: Once = Once::new();
        UI_STATE.call_once(|| {
            let path = std::env::temp_dir().join(format!(
                "roostr-core-synced-tabs-test-{}.json",
                std::process::id()
            ));
            // SAFETY: set once, before any test thread reads it (every test starts here).
            unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
        });
        let mut s = State::new(Arc::new(Handle(vault.clone())), None);
        // The UI state file is the process's: each test starts from its own strips.
        s.ui.set("roostr.tabs.spaces", &saved);
        s.nav = Navigator::new(&s.ui);
        let (tx, woke) = channel();
        let tx = Mutex::new(tx);
        s.rt.set_waker(Arc::new(move || {
            let _ = tx.lock().send(());
        }));
        s.on_store(|s, e| match e {
            StoreEvent::Committed(_) => s.slot::<Heard>().commits += 1,
            StoreEvent::Refreshed => s.slot::<Heard>().refreshes += 1,
            _ => {}
        });
        s.nav.use_space(space, &mut s.ui);
        let mut app = App { s, vault, woke };
        app.until("the first refresh and the commit stream", |s, v| {
            s.store.loaded && v.streams.lock().len() == 1
        });
        app
    }

    /// Poll until `done`, waiting for the executor's wakes in between.
    fn until(&mut self, what: &str, done: impl Fn(&State, &Vault) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.s.poll();
            if done(&self.s, &self.vault) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            let _ = self.woke.recv_timeout(left);
        }
    }

    /// Move the virtual clock by `by` and poll.
    fn advance(&mut self, by: Duration) {
        self.s.rt.advance(by);
        self.s.poll();
    }

    /// The changes so far land (the poll after a handler), then the write
    /// delay passes: `writes` writes of the tabs have happened.
    fn written(&mut self, writes: usize) {
        self.s.poll();
        assert_eq!(
            self.vault.writes(),
            writes - 1,
            "written before the debounce"
        );
        self.advance(TABS_WRITE_DELAY);
        self.until("the tabs' write", |_, v| v.writes() >= writes);
        assert_eq!(self.vault.writes(), writes);
    }

    /// Another device's change: its commit, the throttled refresh, landed.
    fn remote_lands(&mut self, tabs: &[(&str, &[&str])]) {
        let want: BTreeMap<String, Vec<String>> = tabs
            .iter()
            .map(|(space, paths)| {
                let paths = paths.iter().map(|p| p.to_string()).collect();
                ((*space).to_string(), paths)
            })
            .collect();
        let commits = heard(&self.s).0;
        self.vault.remote_write(tabs);
        self.until("the commit", |s, _| heard(s).0 > commits);
        self.advance(THROTTLE);
        self.until("the refresh", |s, _| s.nav.open_tabs() == want);
    }

    fn paths(&self) -> Vec<String> {
        self.s.nav.tabs().iter().map(|t| t.route.path()).collect()
    }
}

fn object(id: &str) -> Route {
    Route::Object(id.into())
}

fn path(id: &str) -> String {
    object(id).path()
}

fn tabs_of(space: &str, ids: &[&str]) -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([(space.to_string(), ids.iter().map(|id| path(id)).collect())])
}

#[test]
fn an_opened_tab_is_written_once_after_the_debounce() {
    let vault = Vault::new(&["sp-open"], &[("o1", "sp-open"), ("o2", "sp-open")]);
    let mut app = App::new(vault.clone(), serde_json::json!({}), "sp-open");
    app.s.nav.navigate(object("o1"), &mut app.s.ui);
    app.s.poll();
    nav::open_tab(object("o2"), true, &mut app.s);
    app.s.poll();
    // Not before the delay since the last change.
    app.advance(TABS_WRITE_DELAY - Duration::from_millis(100));
    assert_eq!(vault.writes(), 0, "written before the debounce");
    app.written(1);
    assert_eq!(vault.stored(), tabs_of("sp-open", &["o1", "o2"]));
    // Nothing changed since: no second write.
    app.advance(TABS_WRITE_DELAY * 3);
    assert_eq!(vault.writes(), 1);
    // The stored shape: space id -> its paths, nothing else.
    let written = vault.tab_writes.lock()[0].clone();
    assert_eq!(
        serde_json::to_value(&written).unwrap(),
        serde_json::json!({ "mapValue": { "entries": { "sp-open": { "valuesValue": { "items": [
            { "stringValue": "/app/object/o1" },
            { "stringValue": "/app/object/o2" },
        ] } } } } })
    );
}

#[test]
fn reordering_and_closing_tabs_are_written_but_selecting_one_is_not() {
    let objects = [("a", "sp-edit"), ("b", "sp-edit"), ("c", "sp-edit")];
    let vault = Vault::new(&["sp-edit"], &objects);
    let mut app = App::new(vault.clone(), serde_json::json!({}), "sp-edit");
    app.s.nav.navigate(object("a"), &mut app.s.ui);
    nav::open_tab(object("b"), true, &mut app.s);
    nav::open_tab(object("c"), true, &mut app.s);
    app.written(1);
    assert_eq!(vault.stored(), tabs_of("sp-edit", &["a", "b", "c"]));

    // Selecting another tab is this device's own.
    app.s.nav.activate(1, &mut app.s.ui);
    app.advance(TABS_WRITE_DELAY * 2);
    assert_eq!(vault.writes(), 1, "the selection was written");

    // Reorder: c first.
    app.s.nav.move_tab(2, 0, &mut app.s.ui);
    app.written(2);
    assert_eq!(vault.stored(), tabs_of("sp-edit", &["c", "a", "b"]));
    // Close a.
    app.s.nav.close(1, &mut app.s.ui);
    app.written(3);
    assert_eq!(vault.stored(), tabs_of("sp-edit", &["c", "b"]));
    assert_eq!(app.s.nav.route(), &object("b"), "the selection stayed");
}

#[test]
fn another_devices_change_lands_in_place_and_keeps_the_selection() {
    let objects = [("x", "sp-remote"), ("y", "sp-remote"), ("z", "sp-remote")];
    let vault = Vault::new(&["sp-remote"], &objects);
    let mut app = App::new(vault.clone(), serde_json::json!({}), "sp-remote");
    app.s.nav.navigate(object("x"), &mut app.s.ui);
    nav::open_tab(object("y"), false, &mut app.s);
    app.written(1);
    assert_eq!(app.s.nav.route(), &object("y"));
    let y_uid = app.s.nav.tabs()[1].uid;

    // Another device opens z before x: the strip follows, y stays selected.
    app.remote_lands(&[(
        "sp-remote",
        &["/app/object/z", "/app/object/x", "/app/object/y"],
    )]);
    assert_eq!(app.paths(), [path("z"), path("x"), path("y")]);
    assert_eq!(app.s.nav.route(), &object("y"));
    assert_eq!(app.s.nav.tabs()[2].uid, y_uid, "the same tab, moved");
    // Landing is not a change of this device's: nothing is written back.
    app.advance(TABS_WRITE_DELAY * 2);
    assert_eq!(vault.writes(), 1);

    // It closes y, the selected tab: its right neighbour... none, so the left.
    app.remote_lands(&[("sp-remote", &["/app/object/z", "/app/object/x"])]);
    assert_eq!(app.paths(), [path("z"), path("x")]);
    assert_eq!(app.s.nav.route(), &object("x"), "the nearest neighbour");

    // Closing the selected middle tab selects its right neighbour.
    app.remote_lands(&[(
        "sp-remote",
        &["/app/object/z", "/app/object/x", "/app/object/y"],
    )]);
    assert_eq!(app.s.nav.route(), &object("x"));
    app.remote_lands(&[("sp-remote", &["/app/object/z", "/app/object/y"])]);
    assert_eq!(app.s.nav.route(), &object("y"), "the right neighbour");
    // Everything closed elsewhere: the space home.
    app.remote_lands(&[]);
    assert_eq!(app.paths(), ["/app"]);
    assert_eq!(app.s.nav.route(), &Route::Home);
    app.advance(TABS_WRITE_DELAY * 2);
    assert_eq!(vault.writes(), 1);
}

#[test]
fn the_first_read_merges_this_devices_tabs_once() {
    let objects = [("r1", "sp-merge"), ("r2", "sp-merge"), ("l1", "sp-merge")];
    let vault = Vault::new(&["sp-merge"], &objects);
    vault.remote_write(&[("sp-merge", &["/app/object/r1", "/app/object/r2"])]);
    // This device's strip from before tabs synced (no `synced` mark).
    let saved = serde_json::json!({
        "sp-merge": { "paths": ["/app/object/l1", "/app/object/r1"], "active": 0 }
    });
    let mut app = App::new(vault.clone(), saved, "sp-merge");
    // The shared order first, then this device's own; r1 once.
    assert_eq!(app.paths(), [path("r1"), path("r2"), path("l1")]);
    assert_eq!(app.s.nav.route(), &object("l1"), "the selection stayed");
    app.written(1);
    assert_eq!(vault.stored(), tabs_of("sp-merge", &["r1", "r2", "l1"]));

    // Another device closes l1 and r1: they stay closed (no second merge).
    app.remote_lands(&[("sp-merge", &["/app/object/r2"])]);
    assert_eq!(app.paths(), [path("r2")]);
    let refreshes = heard(&app.s).1;
    app.s.store.refresh_all();
    app.until("another refresh", |s, _| heard(s).1 > refreshes);
    app.advance(TABS_WRITE_DELAY * 2);
    assert_eq!(app.paths(), [path("r2")]);
    assert_eq!(vault.writes(), 1);
}

#[test]
fn a_binned_objects_tabs_close_everywhere() {
    let objects = [("keep", "sp-bin"), ("gone", "sp-bin"), ("other", "sp-bin2")];
    let vault = Vault::new(&["sp-bin", "sp-bin2"], &objects);
    let mut app = App::new(vault.clone(), serde_json::json!({}), "sp-bin2");
    // A tab of the binned object in another space too.
    app.s.nav.navigate(object("gone"), &mut app.s.ui);
    nav::open_tab(object("other"), true, &mut app.s);
    app.s.nav.use_space("sp-bin", &mut app.s.ui);
    app.s.nav.navigate(object("keep"), &mut app.s.ui);
    nav::open_tab(object("gone"), false, &mut app.s);
    app.written(1);

    let commits = heard(&app.s).0;
    vault.bin("gone");
    app.until("the commit", |s, _| heard(s).0 > commits);
    app.advance(THROTTLE);
    app.until("the tabs closing", |s, _| {
        s.nav.tabs().iter().all(|t| t.route != object("gone"))
    });
    assert_eq!(app.paths(), [path("keep")]);
    assert_eq!(app.s.nav.route(), &object("keep"));
    app.written(2);
    let mut want = tabs_of("sp-bin", &["keep"]);
    want.extend(tabs_of("sp-bin2", &["other"]));
    assert_eq!(vault.stored(), want);
}
