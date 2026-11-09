//! The headless harness the shell's tests run on: a [`State`] over a test's
//! fake backend with the shell registered, in a [`rune::Runtime`] drawing
//! [`roostr_shell::view`]. Async work runs on the executor's worker threads;
//! [`Harness::settle`] waits on the executor's wakes (never sleeps) until
//! nothing is left to do.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use roostr_core::nav::Route;
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap, State};
use roostr_rt::{BoxFuture, BoxStream};
use roostr_shell::{Pages, PanePart};
use roostr_ui::Images;
use rune::{El, Env, Runtime, col, text};
use serde_json::Value as Json;

/// How long [`Harness::settle`] waits for a wake before calling the state idle.
const IDLE: Duration = Duration::from_millis(250);
/// One frame of the virtual clock.
const FRAME: f64 = 1.0 / 60.0;

/// `ROOSTR_UI_STATE` is process-wide: set it only while a state loads.
static UI_STATE_ENV: Mutex<()> = Mutex::new(());

pub struct Harness {
    pub rt: Runtime<State>,
    wakes: Receiver<()>,
    /// The UI state file this state persists to.
    pub ui_path: PathBuf,
    _dir: tempfile::TempDir,
}

/// Plain pages: the object's name and a line of body text, and the pane parts
/// by name (keys `test-page`, `test-pane-chat`, `test-pane-properties`,
/// `test-pane-sheet`).
pub fn test_pages() -> Pages {
    Pages {
        page: |s, _, route, _| page(s, route),
        pane: |_, _, id, part, _| {
            let name = match part {
                PanePart::Column(tab) => tab.name().to_lowercase(),
                PanePart::Sheet => "sheet".to_string(),
            };
            col()
                .key(format!("test-pane-{name}"))
                .w_full()
                .grow(1.)
                .padding(16.)
                .child(text(format!("{name} for {id}")))
        },
        ..Pages::default()
    }
}

/// [`test_pages`] whose object pages publish a pane with a chat (count 3).
pub fn pane_pages() -> Pages {
    Pages {
        shown: |s, route| {
            if let Route::Object(id) = route
                && s.store.summary(id).is_some()
            {
                s.pane.publish(true, true, 3, 1);
            }
        },
        ..test_pages()
    }
}

fn page(s: &State, route: &Route) -> El {
    let name = match route {
        Route::Object(id) => s
            .store
            .summary(id)
            .map(|o| o.name.clone())
            .or_else(|| s.store.space(id).map(|sp| sp.name.clone()))
            .unwrap_or_else(|| id.clone()),
        other => other.path(),
    };
    col()
        .key("test-page")
        .w_full()
        .gap(12.)
        .child(text(name))
        .child(text("A page of the shell's tests."))
}

impl Harness {
    /// The shell at `size` over `backend`, with [`test_pages`] and a fresh UI state.
    pub fn new(backend: Arc<dyn Backend>, size: (f32, f32)) -> Harness {
        Self::build(backend, size, serde_json::json!({}), test_pages())
    }

    /// [`Self::new`] with the UI state file holding `ui` first (stored prefs).
    pub fn with_ui(backend: Arc<dyn Backend>, size: (f32, f32), ui: serde_json::Value) -> Harness {
        Self::build(backend, size, ui, test_pages())
    }

    /// [`Self::new`] with the app's `pages`.
    pub fn with_pages(backend: Arc<dyn Backend>, size: (f32, f32), pages: Pages) -> Harness {
        Self::build(backend, size, serde_json::json!({}), pages)
    }

    pub fn build(
        backend: Arc<dyn Backend>,
        size: (f32, f32),
        ui: serde_json::Value,
        pages: Pages,
    ) -> Harness {
        let dir = tempfile::tempdir().expect("temp dir");
        let ui_path = dir.path().join("ui-state.json");
        std::fs::write(&ui_path, serde_json::to_vec(&ui).expect("json")).expect("ui state");
        let mut s = {
            let _env = UI_STATE_ENV.lock();
            // SAFETY: the lock keeps every harness of this process from touching the
            // variable at once; nothing else in the tests reads it.
            unsafe { std::env::set_var("ROOSTR_UI_STATE", &ui_path) };
            State::new(backend, None)
        };
        let (woke, wakes) = channel::<()>();
        let woke = Mutex::new(woke);
        let waker: rune::Waker = Arc::new(move || {
            let _ = woke.lock().send(());
        });
        s.rt.set_waker(waker.clone());
        s.put(Images::new(waker, 2.0));
        roostr_shell::register(&mut s, pages);
        let rt = Runtime::new(s, roostr_shell::view, size, Env::new(true, 2.0));
        let mut h = Harness {
            rt,
            wakes,
            ui_path,
            _dir: dir,
        };
        h.settle();
        h
    }

    /// One poll of the state and one frame.
    pub fn frame(&mut self) -> bool {
        let ran = self.rt.update_if(|s, _| s.poll().updated);
        self.rt.frame(FRAME);
        ran
    }

    /// Poll and draw until the state has nothing left to do (futures done,
    /// no wake within [`IDLE`]), then land every animation.
    pub fn settle(&mut self) {
        for _ in 0..2_000 {
            while self.wakes.try_recv().is_ok() {}
            let updated = self.frame();
            if updated {
                continue;
            }
            // Layout reads and on_layout handlers settle within a few frames.
            for _ in 0..3 {
                self.frame();
            }
            if self.wakes.recv_timeout(IDLE).is_err() {
                break;
            }
        }
        self.rt.settle_all();
        self.frame();
        self.frame();
    }

    /// Run `frames` frames of animation (virtual time), polling each.
    pub fn frames(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame();
        }
    }

    /// Move the executor's clock (timers) by `by`, then settle.
    pub fn advance(&mut self, by: Duration) {
        self.rt.state.rt.advance(by);
        self.settle();
    }

    /// Change the state from outside a handler (then settle).
    pub fn update<R>(&mut self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut out = None;
        self.rt.update(|s, _| out = Some(f(s)));
        self.settle();
        out.expect("update ran")
    }

    pub fn state(&self) -> &State {
        &self.rt.state
    }

    /// The window is now `w` × `h` points.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.rt.set_viewport((w, h), 2.0);
        self.settle();
    }

    /// An element keyed `key` shows (not exiting).
    pub fn has(&self, key: &str) -> bool {
        self.rt.find(key).is_some()
    }

    /// The drawn rect `[x, y, w, h]` of the element keyed `key`.
    pub fn rect(&self, key: &str) -> Option<[f32; 4]> {
        self.rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
            .map(|e| e.rect)
    }

    /// Every shown key starting with `prefix`, in tree order.
    pub fn keys(&self, prefix: &str) -> Vec<String> {
        self.rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.key)
            .filter(|k| k.starts_with(prefix))
            .collect()
    }

    /// Every text shown, in tree order.
    pub fn texts(&self) -> Vec<String> {
        self.rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.text)
            .collect()
    }

    /// Press and release the element keyed `key` at its centre, then settle.
    pub fn click(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.click_at(x, y);
    }

    /// Press and release at window point (`x`, `y`), then settle.
    pub fn click_at(&mut self, x: f32, y: f32) {
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.frame(FRAME);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    /// Hover the element keyed `key` (the pointer rests at its centre).
    pub fn hover(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.rt.pointer_move(x, y);
        self.frames(2);
    }

    /// Drag from the centre of `key` by (`dx`, `dy`) in `steps` moves, then settle.
    pub fn drag(&mut self, key: &str, dx: f32, dy: f32, steps: usize) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.frame(FRAME);
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            self.rt.pointer_move(x + dx * f, y + dy * f);
            self.frame();
        }
        self.rt.pointer_up(x + dx, y + dy);
        self.settle();
    }

    /// A wheel step of `dy` points over the element keyed `key`, then a few frames.
    pub fn wheel(&mut self, key: &str, dy: f32) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key}"));
        self.rt.pointer_move(x, y);
        self.rt.wheel(x, y, dy);
        self.frames(3);
    }

    /// A key press (with modifiers), then settle.
    pub fn key(&mut self, ev: rune::KeyEvent) {
        self.rt.key(ev);
        self.settle();
    }

    /// Type `text` into the focused field, then settle.
    pub fn type_text(&mut self, text: &str) {
        self.rt.key(rune::KeyEvent {
            key: rune::Key::Text(text.to_string()),
            mods: rune::Mods::default(),
        });
        self.settle();
    }
}

/// One space (`sp-1`, Personal) holding one page (`page-1`, Launch plan).
pub struct OnePage;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

impl Backend for OnePage {
    fn describe(&self) -> String {
        "one page".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: "page-1".into(),
            type_key: "page".into(),
            name: "Launch plan".into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        ready(if id == "page-1" {
            Ok(Object {
                id: id.into(),
                type_key: "page".into(),
                fields: [
                    ("name".to_string(), Value::string("Launch plan")),
                    ("channel".to_string(), Value::string("sp-1")),
                ]
                .into(),
                ..Object::default()
            })
        } else {
            Err(anyhow::anyhow!("unknown object {id}"))
        })
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, _: &Json) -> BoxFuture<Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
}
