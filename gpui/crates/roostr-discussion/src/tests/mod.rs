//! Headless tests of the discussion, ported from the GPUI app's `tests/{agent_chat_tag,
//! chat_agents_live, chat_pane, chat_rows, mention_typed}.rs`, and their harness: a fake
//! vault, the app [`State`] over it in a Rune [`Runtime`], and a view standing in for the
//! shell (object page with its pane, the phone's sheet, the chat page). Async work runs on
//! roostr-rt; background results arrive through the executor's waker, so the harness waits
//! on wakes, never on a clock.

mod agent_chat_tag;
mod chat_agents_live;
mod chat_growth;
mod chat_pane;
mod chat_rows;
mod mention_typed;
mod reactions;

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::nav::Route;
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap, PaneTab, State};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use roostr_ui::ThemePref;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, row};
use serde_json::Value as Json;

use crate::discussion::{Disc, Place, get, instance_key};
use crate::{DiscussionOptions, chat_page, discussion, pane_column, pane_counts, pane_sheet};

/// The tests keep their UI preferences in a temp dir, never the user's.
static UI_STATE: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("temp dir");
    // SAFETY: set once, before any State reads it; every harness forces this first.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", dir.path().join("ui-state.json")) };
    dir
});

/// One space (`sp-1`) of objects, each answered from `objects`; agents are the `agent` ones.
pub struct Vault {
    pub objects: Mutex<Vec<Object>>,
    pub me: String,
    /// (object id, text) of every chat line posted.
    pub posts: Mutex<Vec<(String, String)>>,
    /// Every `chat_post`'s params, in order.
    pub chat_posts: Mutex<Vec<JsonMap>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
    sender: mpsc::UnboundedSender<anyhow::Result<BackendEvent>>,
}

impl Vault {
    pub fn new(objects: Vec<Object>, me: &str) -> Arc<Vault> {
        let (sender, rx) = mpsc::unbounded();
        Arc::new(Vault {
            objects: Mutex::new(objects),
            me: me.into(),
            posts: Mutex::new(Vec::new()),
            chat_posts: Mutex::new(Vec::new()),
            events: Mutex::new(Some(rx)),
            sender,
        })
    }

    /// The daemon reports a commit to `id`.
    pub fn commit(&self, id: &str) {
        self.sender
            .unbounded_send(Ok(BackendEvent::Committed(id.into())))
            .expect("the store reads its stream");
    }

    pub fn posts(&self) -> Vec<(String, String)> {
        self.posts.lock().clone()
    }

    pub fn update(&self, id: &str, edit: impl FnOnce(&mut Object)) {
        let mut objects = self.objects.lock();
        let object = objects
            .iter_mut()
            .find(|o| o.id == id)
            .expect("object in the vault");
        edit(object);
    }
}

/// An object of `type_key` named `name` in space `sp-1`.
pub fn object(id: &str, type_key: &str, name: &str) -> Object {
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields: [
            ("name".to_string(), Value::string(name)),
            ("channel".to_string(), Value::string("sp-1")),
        ]
        .into(),
        ..Object::default()
    }
}

/// `agents` as `object`'s Agent property.
pub fn with_guests(mut object: Object, agents: &[&str]) -> Object {
    object.fields.insert(
        "agent".into(),
        Value::values(agents.iter().map(|a| Value::link(*a, "agent")).collect()),
    );
    object
}

pub fn post(object_id: &str, text: &str) -> (String, String) {
    (object_id.to_string(), text.to_string())
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a test vault".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        let me = self.me.clone();
        boxed(async move {
            Ok(Settings {
                has_key: true,
                author_id: me,
                ..Settings::default()
            })
        })
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        let rows = self
            .objects
            .lock()
            .iter()
            .map(|o| ObjectSummary {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: o.name().to_string(),
                channel_id: "sp-1".into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect();
        boxed(async move { Ok(rows) })
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let found = self.objects.lock().iter().find(|o| o.id == id).cloned();
        let id = id.to_string();
        boxed(async move { found.ok_or_else(|| anyhow::anyhow!("unknown object {id}")) })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        boxed(async {
            Ok(vec![Space {
                id: "sp-1".into(),
                name: "Personal".into(),
                created_at: 1,
                ..Space::default()
            }])
        })
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        boxed(async { Ok(Vec::new()) })
    }
    /// Objects of the body's type (the space's agents for `loadObjectAgents` and the store's
    /// roster, its Reaction objects for the reaction bar); nothing else is asked.
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let records: Vec<QueryRow> = self
            .objects
            .lock()
            .iter()
            .filter(|o| body["type"] == o.type_key.as_str())
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                fields: o.fields.clone(),
                ..QueryRow::default()
            })
            .collect();
        boxed(async move {
            Ok(QueryResult {
                total: records.len() as _,
                records,
            })
        })
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "chat_post" {
            let field = |key: &str| params[key].as_str().unwrap_or_default().to_string();
            self.posts.lock().push((field("object_id"), field("text")));
            self.chat_posts.lock().push(params.clone());
        }
        boxed(async { Ok(JsonMap::new()) })
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        match self.events.lock().take() {
            Some(rx) => boxed_stream(rx),
            None => boxed_stream(futures::stream::pending()),
        }
    }
}

/// The window size the test view lays out for (the shell knows it from the window).
struct Window([f32; 2]);

/// What the shell would show: the chat page for `Route::Chat`; for an object, on a phone its
/// page (the collapsed opener) or the pane's sheet when open, on a desktop the page with the
/// inline discussion beside the open pane columns.
fn shell(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = Vec::new();
    let window = s.get::<Window>().map_or([1280., 800.], |w| w.0);
    let mobile = roostr_ui::layout::is_mobile(window[0]);
    let base = match s.nav.route().clone() {
        Route::Chat(id) => chat_page(s, cx, t, &mut layer, &id, window),
        Route::Object(id) => {
            let chat = s.store.object(&id).is_some_and(|o| pane_counts(&o).1 > 0);
            if mobile && s.pane.sheet {
                pane_sheet(s, cx, t, &mut layer, &id, chat, window)
            } else {
                let page = col()
                    .key("page")
                    .grow(1.)
                    .min_w(0.)
                    .h_full()
                    .child(discussion(
                        s,
                        cx,
                        t,
                        &mut layer,
                        &id,
                        &DiscussionOptions::default(),
                        window,
                    ));
                let mut frame = row().w_full().h_full().child(page);
                if !mobile {
                    for tab in [PaneTab::Chat, PaneTab::Properties] {
                        if s.pane.is_open(tab) && (chat || tab == PaneTab::Properties) {
                            frame =
                                frame.child(col().w(s.pane.width(tab)).h_full().ml(6.).child(
                                    pane_column(s, cx, t, &mut layer, &id, chat, tab, window),
                                ));
                        }
                    }
                }
                frame
            }
        }
        _ => col(),
    };
    roostr_ui::root(col().w_full().h_full().fill(t.bg).child(base), layer)
}

/// Publishes the open object's pane counts, as the object page does.
fn publish(s: &mut State) -> bool {
    let Some(id) = s.nav.route().object_id().map(str::to_string) else {
        return false;
    };
    let Some(object) = s.store.object(&id) else {
        return false;
    };
    let (count, conversations) = pane_counts(&object);
    let p = &s.pane;
    if p.available && p.count == count && p.conv_count == conversations {
        return false;
    }
    s.pane
        .publish(true, conversations > 0, count, conversations);
    true
}

pub struct Harness {
    pub rt: Runtime<State>,
    wakes: Receiver<()>,
}

impl Harness {
    /// The app over `vault` in a `size` window, its first load done.
    pub fn new(vault: Arc<Vault>, size: (f32, f32)) -> Harness {
        LazyLock::force(&UI_STATE);
        let mut s = State::new(vault, None);
        crate::register(&mut s);
        s.on_poll(publish);
        s.put(Window([size.0, size.1]));
        let (tx, wakes) = channel::<()>();
        let tx = Mutex::new(tx);
        s.rt.set_waker(Arc::new(move || {
            let _ = tx.lock().send(());
        }));
        let mut rt = Runtime::new(s, shell, size, Env::new(true, 2.0));
        rt.poll = Some(roostr_host::poll);
        let mut h = Harness { rt, wakes };
        h.settle();
        h
    }

    /// Frames until nothing more arrives: futures woken by background work (the fake
    /// vault's answers run on worker threads) and what they start.
    pub fn settle(&mut self) {
        for _ in 0..200 {
            self.frame();
            self.frame();
            match self.wakes.recv_timeout(Duration::from_millis(200)) {
                Ok(()) => while self.wakes.try_recv().is_ok() {},
                Err(_) => break,
            }
        }
        // Motion lands: a second and a half, as a long message rewrapping much taller glides
        // its growth row open for longer than the spring's own 0.32 s.
        for _ in 0..90 {
            self.frame();
        }
    }

    pub fn frame(&mut self) {
        self.rt.frame(1.0 / 60.0);
    }

    /// Frames until `ready` holds, waiting for background work between them, and no motion
    /// frames past it: what just mounted is caught at the start of its animation.
    pub fn frames_until(&mut self, what: &str, ready: impl Fn(&Harness) -> bool) {
        for _ in 0..200 {
            self.frame();
            if ready(self) {
                return;
            }
            let _ = self.wakes.recv_timeout(Duration::from_millis(50));
        }
        panic!("{what} never happened");
    }

    /// Moves the executor's clock (timers) forward, then settles.
    pub fn advance(&mut self, by: Duration) {
        self.rt.state.rt.advance(by);
        self.settle();
    }

    pub fn update<R>(&mut self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut out = None;
        self.rt.update(|s, _| out = Some(f(s)));
        self.settle();
        out.expect("the update ran")
    }

    pub fn navigate(&mut self, route: Route) {
        self.update(|s| roostr_core::nav::navigate(route, s));
    }

    pub fn resize(&mut self, w: f32, h: f32) {
        self.rt.update(|s, _| s.put(Window([w, h])));
        self.rt.set_viewport((w, h), 2.0);
        self.settle();
    }

    pub fn has(&self, key: &str) -> bool {
        self.rt.find(key).is_some()
    }

    /// Drawn rect `[x, y, w, h]` of `key`.
    pub fn rect(&self, key: &str) -> Option<[f32; 4]> {
        self.rt.presented(key).map(|(r, _)| r)
    }

    pub fn key(&mut self, key: Key, mods: Mods) {
        self.rt.key(KeyEvent { key, mods });
        self.frame();
    }

    /// Types `text` into the focused field, one character at a time.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(Key::Text(c.to_string()), Mods::default());
        }
        self.settle();
    }

    /// Types `text` into the focused composer and sends it.
    pub fn send(&mut self, text: &str) {
        self.type_text(text);
        self.key(Key::Enter, Mods::default());
        self.settle();
    }

    /// ⌘A, then Backspace: the focused field emptied.
    pub fn clear_field(&mut self) {
        let cmd = Mods {
            cmd: true,
            logo: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            ..Mods::default()
        };
        self.key(Key::Command('a'), cmd);
        self.key(Key::Backspace, Mods::default());
        self.settle();
    }
}

/// A chat line under `__discussion__` as the harness writes them.
pub fn chat_block(id: &str, author: &str, ts: i64, text: &str) -> Json {
    serde_json::json!({
        "id": id,
        "childrenIds": [],
        "content": { "custom": { "contentType": "chat", "meta": {
            "author": author, "ts": ts.to_string(), "text": text,
        } } },
    })
}

/// `object` with `messages` as its discussion.
pub fn with_discussion(mut object: Object, messages: Vec<Json>) -> Object {
    let ids: Vec<&Json> = messages.iter().map(|m| &m["id"]).collect();
    let mut blocks =
        vec![serde_json::json!({ "id": "__discussion__", "childrenIds": ids, "content": {} })];
    blocks.extend(messages.iter().cloned());
    object.blocks = serde_json::from_value(Json::Array(blocks)).expect("blocks fixture");
    object
}

/// The shown object's discussion at `place`.
pub fn shown(h: &Harness, place: Place) -> Option<&Disc> {
    let s = &h.rt.state;
    let id = s.nav.route().object_id()?;
    get(s, &instance_key(id, place))
}

/// That discussion's composer addresses agent `name`: `@name` in the draft, drawn as a pill.
pub fn tagged(h: &Harness, place: Place, name: &str) -> bool {
    shown(h, place).is_some_and(|d| {
        d.draft.contains(&format!("@{name}"))
            && d.edit
                .pills()
                .any(|(range, id)| id.starts_with("agent:") && d.edit.text[range].ends_with(name))
    })
}
