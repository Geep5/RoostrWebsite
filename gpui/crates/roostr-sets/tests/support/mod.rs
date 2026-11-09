//! Headless test support: a fake vault [`Backend`] the tests fill and read back, and an
//! [`App`] that shows one data view in a page-like scroll column of a Rune [`Runtime`]
//! over it. Waits only on the executor's wakes (worker threads answering), never sleeps;
//! timers run on the executor's virtual clock ([`App::advance`]).
#![allow(dead_code)]

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Route, State};
use roostr_rt::{BoxFuture, BoxStream};
use roostr_sets::DataViewKind;
use roostr_ui::ThemePref;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, col, el};
use serde_json::Value as Json;

pub const SPACE: &str = "sp-1";

pub fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

/// An object of `type_key` in [`SPACE`] with `pairs` as its fields.
pub fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    fields
        .entry("channel".into())
        .or_insert_with(|| Value::string(SPACE));
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

/// A relation of [`SPACE`].
pub fn relation(id: &str, key: &str, format: &str, name: &str) -> RelationDef {
    RelationDef {
        id: id.into(),
        key: key.into(),
        format: format.into(),
        name: name.into(),
        space: Some(SPACE.into()),
        ..RelationDef::default()
    }
}

/// A `Value` from its wire JSON (`{"valuesValue": …}`).
pub fn value(json: Json) -> Value {
    serde_json::from_value(json).expect("a wire value")
}

type QueryFn = Box<dyn Fn(&Vault, &Json) -> Vec<QueryRow> + Send + Sync>;

/// The fake vault: objects, relations and one space; every mutation recorded and
/// `set_field` written. `on_query` picks the rows a query answers.
pub struct Vault {
    pub objects: Mutex<Vec<Object>>,
    pub relations: Mutex<Vec<RelationDef>>,
    pub mutations: Mutex<Vec<(String, JsonMap)>>,
    pub on_query: QueryFn,
}

impl Vault {
    pub fn new(objects: Vec<Object>, relations: Vec<RelationDef>, on_query: QueryFn) -> Self {
        Self {
            objects: Mutex::new(objects),
            relations: Mutex::new(relations),
            mutations: Mutex::new(Vec::new()),
            on_query,
        }
    }

    pub fn object(&self, id: &str) -> Option<Object> {
        self.objects.lock().iter().find(|o| o.id == id).cloned()
    }

    pub fn field(&self, id: &str, key: &str) -> Option<Value> {
        self.object(id).and_then(|o| o.fields.get(key).cloned())
    }

    /// Every object of `type_key`, as query rows.
    pub fn of_type(&self, type_key: &str) -> Vec<QueryRow> {
        self.objects
            .lock()
            .iter()
            .filter(|o| o.type_key == type_key)
            .map(Vault::row)
            .collect()
    }

    pub fn row(o: &Object) -> QueryRow {
        QueryRow {
            id: o.id.clone(),
            type_key: o.type_key.clone(),
            name: Some(o.field_str("name").into()),
            fields: o.fields.clone(),
            created_at: o.created_at,
            updated_at: o.updated_at,
            ..QueryRow::default()
        }
    }
}

/// The vault behind an `Arc`, as `State::new` takes it.
pub struct Shared(pub Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        "the test vault".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .0
            .objects
            .lock()
            .iter()
            .map(|o| ObjectSummary {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: o.field_str("name").into(),
                channel_id: o.field_str("channel").into(),
                updated_at: o.updated_at.max(1_000),
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let object = self.0.object(id);
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(self.0.relations.lock().clone()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let mut records = (self.0.on_query)(&self.0, body);
        // Like the store: a query that names its fields gets only those.
        if let Some(keys) = body["fields"].as_array() {
            for r in &mut records {
                r.fields.retain(|k, _| keys.iter().any(|f| f == k.as_str()));
            }
        }
        let total = records.len();
        let offset = body["offset"].as_u64().unwrap_or(0) as usize;
        let limit = body["limit"].as_u64().map_or(usize::MAX, |l| l as usize);
        let records = records.into_iter().skip(offset).take(limit).collect();
        ready(Ok(QueryResult {
            total: total as u64,
            records,
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        self.0
            .mutations
            .lock()
            .push((action.to_string(), params.clone()));
        if action == "set_field" {
            let id = params["object_id"].as_str().unwrap_or_default().to_string();
            let key = params["key"].as_str().unwrap_or_default().to_string();
            let value: Value = serde_json::from_value(params["value"].clone()).expect("a value");
            if let Some(o) = self.0.objects.lock().iter_mut().find(|o| o.id == id) {
                o.fields.insert(key, value);
                o.updated_at += 1;
            }
        }
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
}

/// Which data view the test window shows.
struct Shown {
    id: String,
    kind: DataViewKind,
}

/// The page: a vertical scroll column with the data view in it (as the object
/// page hosts it), overlays mounted at the root.
fn page(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let Some(shown) = s.get::<Shown>() else {
        return el();
    };
    let mut layer = Vec::new();
    let view = roostr_sets::view(s, cx, t, &mut layer, &shown.id, shown.kind);
    let page = col()
        .key("page")
        .w_full()
        .h_full()
        .fill(t.bg)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .px(32.0)
        .pt(24.0)
        .pb(80.0)
        .child(view);
    roostr_ui::root(page, layer)
}

/// A data view in a headless window over a [`Vault`].
pub struct App {
    pub rt: Runtime<State>,
    pub vault: Arc<Vault>,
    woke: Receiver<()>,
}

impl App {
    /// The state over `vault` with this crate and roostr-properties registered, its first
    /// load landed, showing the data view of `id` in a `size` dark window.
    pub fn new(vault: Vault, id: &str, kind: DataViewKind, size: (f32, f32)) -> App {
        static UI_STATE: Once = Once::new();
        UI_STATE.call_once(|| {
            let path =
                std::env::temp_dir().join(format!("roostr-sets-test-{}.json", std::process::id()));
            // SAFETY: set once, before any test thread reads it (every test starts here).
            unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
        });
        let vault = Arc::new(vault);
        let mut s = State::new(Arc::new(Shared(vault.clone())), None);
        let (tx, woke) = channel();
        let tx = Mutex::new(tx);
        s.rt.set_waker(Arc::new(move || {
            let _ = tx.lock().send(());
        }));
        roostr_sets::register(&mut s);
        roostr_properties::register(&mut s);
        s.put(Shown {
            id: id.to_string(),
            kind,
        });
        let rt = Runtime::new(s, page, size, Env::new(true, 2.0));
        let mut app = App { rt, vault, woke };
        app.until("the first load", |s| s.store.loaded);
        let id = id.to_string();
        app.until("the view's object", move |s| s.store.object(&id).is_some());
        app
    }

    pub fn s(&mut self) -> &mut State {
        &mut self.rt.state
    }

    /// One poll and one frame.
    pub fn step(&mut self) {
        self.rt.update(|s, _| {
            s.poll();
        });
        self.rt.frame(1.0 / 60.0);
        // Land layout springs, so rects are where they rest.
        self.rt.frame(1.0);
    }

    /// Polls and draws until `done` holds, waiting for the executor's wakes in between
    /// (worker threads answering the fake backend). Panics after 10 s.
    pub fn until(&mut self, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.step();
            if done(&self.rt.state) {
                self.step();
                self.step();
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            let _ = self.woke.recv_timeout(left.min(Duration::from_millis(50)));
        }
    }

    /// Waits until the element keyed `key` is drawn.
    pub fn until_shown(&mut self, key: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.step();
            if self.shown(key) {
                self.step();
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let keys: Vec<String> = self
                    .rt
                    .elements()
                    .into_iter()
                    .filter_map(|e| e.key)
                    .filter(|k| !k.starts_with('#'))
                    .filter(|k| {
                        let probe = key.split('/').next().unwrap_or(key);
                        k.contains(probe) || k.contains("pop") || k.contains("scrim")
                    })
                    .take(80)
                    .collect();
                panic!("timed out waiting for {key} to show; drawn: {keys:?}");
            }
            let _ = self.woke.recv_timeout(left.min(Duration::from_millis(50)));
        }
    }

    /// Moves the executor's clock (debounces, timers) and draws.
    pub fn advance(&mut self, by: Duration) {
        self.rt.state.rt.advance(by);
        self.step();
        self.step();
    }

    pub fn shown(&self, key: &str) -> bool {
        self.rect(key).is_some()
    }

    /// The window rect `[x, y, w, h]` of the element keyed `key` (or key path).
    /// A key path `scope/leaf` (roostr-properties scopes its parts under the builder's key)
    /// finds the element keyed `leaf` inside the one keyed `scope`.
    pub fn rect(&self, key: &str) -> Option<[f32; 4]> {
        let elements = self.rt.elements();
        if let Some(e) = elements.iter().find(|e| e.key.as_deref() == Some(key)) {
            return Some(e.rect);
        }
        let (scope, leaf) = key.rsplit_once('/')?;
        elements
            .iter()
            .find(|e| e.key.as_deref() == Some(leaf) && e.path.contains(scope))
            .map(|e| e.rect)
    }

    /// Presses at a window point.
    pub fn click_at(&mut self, x: f32, y: f32) {
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.step();
    }

    /// Presses the element keyed `key` (or key path) at its centre.
    pub fn click(&mut self, key: &str) {
        let r = self
            .rect(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.click_at(r[0] + r[2] / 2., r[1] + r[3] / 2.);
    }

    pub fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        self.step();
    }

    /// Types `text` into the focused field.
    pub fn type_text(&mut self, text: &str) {
        self.key(Key::Text(text.into()));
    }

    /// The route the navigator shows.
    pub fn route(&self) -> Route {
        self.rt.state.nav.route().clone()
    }
}
