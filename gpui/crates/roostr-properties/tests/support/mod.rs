//! Headless test support: a fake vault [`Backend`] the tests fill and read back, and an
//! [`App`] that runs a view of this crate in a Rune [`Runtime`] over it. Waits only on the
//! executor's wakes (worker threads answering), never sleeps; timers run on the executor's
//! virtual clock ([`App::advance`]).
#![allow(dead_code)]

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_rt::{BoxFuture, BoxStream};
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime};
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

type MutateFn = Box<dyn Fn(&Vault, &str, &JsonMap) -> Option<JsonMap> + Send + Sync>;
type QueryFn = Box<dyn Fn(&Vault, &Json) -> Option<QueryResult> + Send + Sync>;

/// The fake vault: objects, relations and one space; every mutation recorded. `set_field`
/// writes the field; a test's `on_mutate` / `on_query` answers the rest (None = default).
#[derive(Default)]
pub struct Vault {
    pub objects: Mutex<Vec<Object>>,
    pub relations: Mutex<Vec<RelationDef>>,
    /// Every mutation in call order: (action, params).
    pub mutations: Mutex<Vec<(String, JsonMap)>>,
    pub on_mutate: Mutex<Option<MutateFn>>,
    pub on_query: Mutex<Option<QueryFn>>,
}

impl Vault {
    pub fn new(objects: Vec<Object>, relations: Vec<RelationDef>) -> Self {
        Self {
            objects: Mutex::new(objects),
            relations: Mutex::new(relations),
            ..Self::default()
        }
    }

    pub fn object(&self, id: &str) -> Option<Object> {
        self.objects.lock().iter().find(|o| o.id == id).cloned()
    }

    pub fn field(&self, id: &str, key: &str) -> Option<Value> {
        self.object(id).and_then(|o| o.fields.get(key).cloned())
    }

    /// The params of every recorded `action`.
    pub fn calls(&self, action: &str) -> Vec<JsonMap> {
        self.mutations
            .lock()
            .iter()
            .filter(|(a, _)| a == action)
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn row(o: &Object) -> QueryRow {
        QueryRow {
            id: o.id.clone(),
            type_key: o.type_key.clone(),
            name: Some(o.field_str("name").into()),
            fields: o.fields.clone(),
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
        if let Some(f) = self.0.on_query.lock().as_ref()
            && let Some(answer) = f(&self.0, body)
        {
            return ready(Ok(answer));
        }
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .0
            .objects
            .lock()
            .iter()
            .filter(|o| text.contains(&format!("\"{}\"", o.type_key)) || text.contains("setId"))
            .map(Vault::row)
            .collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        self.0
            .mutations
            .lock()
            .push((action.to_string(), params.clone()));
        if let Some(f) = self.0.on_mutate.lock().as_ref()
            && let Some(answer) = f(&self.0, action, &params)
        {
            return ready(Ok(answer));
        }
        if action == "set_field" {
            let id = params["object_id"].as_str().unwrap_or_default().to_string();
            let key = params["key"].as_str().unwrap_or_default().to_string();
            let value: Value = serde_json::from_value(params["value"].clone()).unwrap_or_default();
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

/// A view of this crate in a headless window over a [`Vault`].
pub struct App {
    pub rt: Runtime<State>,
    pub vault: Arc<Vault>,
    woke: Receiver<()>,
}

impl App {
    /// The state over `vault` with this crate registered, its first load landed, drawing
    /// `view` in a 1400×900 dark window.
    pub fn new(vault: Vault, view: fn(&State, &mut Cx<State>) -> El) -> App {
        App::sized(vault, view, (1400.0, 900.0))
    }

    /// [`App::new`] in a window of `size` points.
    pub fn sized(vault: Vault, view: fn(&State, &mut Cx<State>) -> El, size: (f32, f32)) -> App {
        static UI_STATE: Once = Once::new();
        UI_STATE.call_once(|| {
            let path = std::env::temp_dir().join(format!(
                "roostr-properties-test-{}.json",
                std::process::id()
            ));
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
        roostr_properties::register(&mut s);
        let rt = Runtime::new(s, view, size, Env::new(true, 2.0));
        let mut app = App { rt, vault, woke };
        app.until("the first load", |s| s.store.loaded);
        app
    }

    pub fn s(&mut self) -> &mut State {
        &mut self.rt.state
    }

    /// One poll (re-running the view when it changed anything, as the shell does) and one
    /// frame.
    pub fn step(&mut self) {
        if self.rt.state.poll().updated {
            self.rt.invalidate();
        }
        self.rt.frame(1.0 / 60.0);
    }

    /// Polls and draws until `done` holds, waiting for the executor's wakes in between
    /// (worker threads answering the fake backend). Panics after 10 s.
    pub fn until(&mut self, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.step();
            if done(&self.rt.state) {
                // Land what the last poll changed.
                self.step();
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            let _ = self.woke.recv_timeout(left);
        }
    }

    /// Moves the executor's clock (store throttles, polls, timers) and draws.
    pub fn advance(&mut self, by: Duration) {
        self.rt.state.rt.advance(by);
        self.step();
        self.step();
    }

    pub fn shown(&self, key: &str) -> bool {
        self.rt.find(key).is_some()
    }

    /// Presses the element keyed `key` (or key path) at its centre.
    pub fn click(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.step();
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
}
