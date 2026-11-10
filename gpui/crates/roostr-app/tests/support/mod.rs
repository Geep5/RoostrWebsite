//! The assembled app in a headless Rune window over a test vault: what every
//! test of this crate drives (pointer, keys, the clipboard) and reads (element
//! rects by key, the state).

#![allow(dead_code)]

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Once};
use std::time::Duration;

use parking_lot::Mutex;
use roostr_core::{Backend, State};
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime};

/// A frame at 60 Hz.
const FRAME: f64 = 1.0 / 60.0;

/// The app's device-local UI state goes to a scratch file, once per test binary.
fn scratch_ui_state() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let path = std::env::temp_dir().join(format!(
            "roostr-app-test-{}-ui-state.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        // SAFETY: set once, before any app of this binary reads it.
        unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
    });
}

pub struct App {
    pub rt: Runtime<State>,
    woke: Receiver<()>,
}

impl App {
    /// The whole app (every feature registered, the shell's window) over
    /// `backend` in a `width` x `height` window, settled.
    pub fn open(backend: Arc<dyn Backend>, width: f32, height: f32) -> App {
        Self::open_with(backend, width, height, |_| {})
    }

    /// [`Self::open`], with `prepare` run on the state before the first frame.
    pub fn open_with(
        backend: Arc<dyn Backend>,
        width: f32,
        height: f32,
        prepare: impl FnOnce(&mut State),
    ) -> App {
        Self::open_view(backend, width, height, prepare, roostr_shell::view)
    }

    /// [`Self::open_with`], the window drawing `view` (the shell's view
    /// wrapped in a test's own elements).
    pub fn open_view(
        backend: Arc<dyn Backend>,
        width: f32,
        height: f32,
        prepare: impl FnOnce(&mut State),
        view: fn(&State, &mut Cx<State>) -> El,
    ) -> App {
        scratch_ui_state();
        let mut s = State::new(backend, None);
        let (tx, woke) = channel();
        let tx = Mutex::new(tx);
        roostr_host::setup(
            &mut s,
            Arc::new(move || {
                let _ = tx.lock().send(());
            }),
        );
        roostr_app::register(&mut s);
        prepare(&mut s);
        let env = Env {
            system_dark: true,
            scale: 2.0,
        };
        let rt = Runtime::new(s, view, (width, height), env);
        let mut app = App { rt, woke };
        app.settle();
        app
    }

    /// The state, for a change outside any handler (the next frame re-runs the view).
    pub fn state(&mut self) -> &mut State {
        self.rt.invalidate();
        &mut self.rt.state
    }

    /// Runs the state's futures and frames until nothing changes for a
    /// while: loads landed, listeners ran, the view re-ran, motion landed.
    pub fn settle(&mut self) {
        let mut quiet = 0;
        for _ in 0..400 {
            // The window's poll hook, as a running app has it.
            let updated = self.rt.update_if(|s, tx| roostr_host::poll(s, 0., tx));
            self.rt.frame(FRAME);
            if updated {
                quiet = 0;
                continue;
            }
            match self.woke.recv_timeout(Duration::from_millis(40)) {
                Ok(()) => quiet = 0,
                Err(_) => quiet += 1,
            }
            if quiet >= 3 {
                break;
            }
        }
        // Springs and fades land.
        for _ in 0..4 {
            self.rt.frame(0.5);
        }
    }

    /// The element keyed `key` as drawn: `[x, y, w, h]`.
    pub fn rect(&self, key: &str) -> Option<[f32; 4]> {
        let (rect, opacity) = self.rt.presented(key)?;
        (opacity > 0.0 && self.rt.find(key).is_some()).then_some(rect)
    }

    pub fn shown(&self, key: &str) -> bool {
        self.rect(key).is_some()
    }

    fn center(&self, key: &str) -> (f32, f32) {
        self.rt.find(key).unwrap_or_else(|| {
            let keys: Vec<String> = self
                .rt
                .elements()
                .into_iter()
                .filter_map(|e| e.key)
                .collect();
            panic!(
                "{key} is not on screen (route {:?}); keys: {keys:?}",
                self.rt.state.nav.route()
            )
        })
    }

    /// A click at the middle of `key`.
    pub fn tap(&mut self, key: &str) {
        let (x, y) = self.center(key);
        self.click_at(x, y);
    }

    pub fn click_at(&mut self, x: f32, y: f32) {
        self.rt.pointer_move(x, y);
        self.rt.frame(FRAME);
        self.rt.pointer_down(x, y);
        self.rt.frame(FRAME);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    /// The pointer comes to rest over `key`.
    pub fn hover(&mut self, key: &str) {
        let (x, y) = self.center(key);
        self.rt.pointer_move(x, y);
        self.settle();
    }

    /// A press at `from`, a drag to `to`, a release there.
    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.rt.pointer_move(from.0, from.1);
        self.rt.frame(FRAME);
        self.rt.pointer_down(from.0, from.1);
        self.rt.frame(FRAME);
        let steps = 6;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            self.rt
                .pointer_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            self.rt.frame(FRAME);
        }
        self.rt.pointer_up(to.0, to.1);
        self.settle();
    }

    pub fn type_text(&mut self, text: &str) {
        self.key(Key::Text(text.into()));
    }

    pub fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        self.settle();
    }

    pub fn navigate(&mut self, route: roostr_core::nav::Route) {
        roostr_core::nav::navigate(route, self.state());
        self.settle();
    }
}
