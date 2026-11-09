//! The app state Rune hands every view and handler: the model of every
//! former GPUI global of the app, as plain fields, plus typed slots for the
//! feature crates' own state.
//!
//! Async work runs on [`State::rt`]; [`State::poll`] (Rune's poll hook)
//! runs it, then hands the events the store, the navigator and the cover
//! asks queued meanwhile to the listeners registered with [`State::on_store`],
//! [`State::on_nav`] and [`State::on_cover_ask`].

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use roostr_rt::{AsyncCx, Executor, Ran};

use crate::backend::Backend;
use crate::cover::{CoverAsk, CoverAsks};
use crate::create::{AddFile, PickFile};
use crate::harness::Harness;
use crate::layout::SyncNow;
use crate::nav::{NavEvent, Navigator};
use crate::pane::PaneState;
use crate::profile::{self, ProfileCache};
use crate::store::{Store, StoreEvent};
use crate::ui_state::UiState;

/// What async code holds: `cx.update(|s| ..)` reaches the [`State`].
pub type Cx = AsyncCx<State>;

/// Rounds of "run futures, hand out events" one [`State::poll`] does before
/// leaving the rest to the next poll, so listeners that keep emitting cannot
/// freeze the window.
const DISPATCH_ROUNDS: usize = 16;

pub struct State {
    /// Runs the app's futures (`s.rt.spawn(async move |cx| ..)`).
    pub rt: Executor<State>,
    /// The vault's data, kept live by the backend's commit stream.
    pub store: Store,
    /// Routes, tabs and history.
    pub nav: Navigator,
    /// The object page's Chat & Properties pane.
    pub pane: PaneState,
    /// Device-local UI preferences, persisted.
    pub ui: UiState,
    /// The agent harness on this computer, when the backend is its daemon.
    pub harness: Option<Arc<Harness>>,
    /// This identity's profile (name, avatar).
    pub profile: ProfileCache,
    /// The phone menu's requests to open an object page's cover menu.
    pub covers: CoverAsks,
    /// How a picked file becomes a File object; installed by the editor.
    pub add_file: Option<AddFile>,
    /// The host's file dialog; installed by the app (no picker: no file adds).
    pub pick_file: Option<PickFile>,
    /// The host's "sync now", where it has one (iOS).
    pub sync_now: Option<SyncNow>,
    /// Spaces whose lite download this session already asked for
    /// ([`crate::lite_ui::load_space`]).
    pub(crate) lite_requested: HashSet<String>,
    slots: HashMap<TypeId, Box<dyn Any>>,
    store_listeners: Vec<fn(&mut State, &StoreEvent)>,
    nav_listeners: Vec<fn(&mut State, &NavEvent)>,
    cover_listeners: Vec<fn(&mut State, &CoverAsk)>,
}

impl State {
    /// The app over `backend`, with the UI state stored on this device.
    /// Starts loading: settings, then the collections and the commit stream,
    /// and the profile when there is a harness or an on-device account.
    /// `harness` is the computer's agent harness, when the backend is a
    /// local daemon.
    pub fn new(backend: Arc<dyn Backend>, harness: Option<Arc<Harness>>) -> State {
        let rt = Executor::new();
        let ui = UiState::load();
        let mut s = State {
            store: Store::new(backend, rt.clone()),
            nav: Navigator::new(&ui),
            pane: PaneState::new(&ui),
            ui,
            rt,
            harness,
            profile: ProfileCache::default(),
            covers: CoverAsks::default(),
            add_file: None,
            pick_file: None,
            sync_now: None,
            lite_requested: HashSet::new(),
            slots: HashMap::new(),
            store_listeners: Vec::new(),
            nav_listeners: Vec::new(),
            cover_listeners: Vec::new(),
        };
        s.store.start();
        if s.harness.is_some() || crate::account::account(&s).is_some() {
            let fetch = profile::fetch(&mut s);
            s.rt.spawn(async move |_| {
                if let Err(error) = fetch.await {
                    log::debug!("profile: {error:#}");
                }
            })
            .detach();
        }
        s
    }

    /// A feature crate's own state, created on first use.
    pub fn slot<T: Default + 'static>(&mut self) -> &mut T {
        self.slots
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut()
            .expect("slot holds its own type")
    }

    /// A feature crate's state, if it was ever created.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.slots.get(&TypeId::of::<T>())?.downcast_ref()
    }

    /// Call `f` with every [`StoreEvent`] (what `cx.subscribe(&store, ..)` did).
    pub fn on_store(&mut self, f: fn(&mut State, &StoreEvent)) {
        self.store_listeners.push(f);
    }

    /// Call `f` with every [`NavEvent`].
    pub fn on_nav(&mut self, f: fn(&mut State, &NavEvent)) {
        self.nav_listeners.push(f);
    }

    /// Call `f` with every [`CoverAsk`] ([`crate::cover::ask`]).
    pub fn on_cover_ask(&mut self, f: fn(&mut State, &CoverAsk)) {
        self.cover_listeners.push(f);
    }

    /// Rune's poll hook: runs the futures that can move, then hands the
    /// queued events to their listeners (and runs what those started).
    /// `updated` when anything may have changed what the view shows.
    pub fn poll(&mut self) -> Ran {
        let rt = self.rt.clone();
        let mut ran = rt.run(self);
        for _ in 0..DISPATCH_ROUNDS {
            if !self.dispatch() {
                return ran;
            }
            let next = rt.run(self);
            ran = Ran {
                updated: true,
                next_timer: next.next_timer,
            };
        }
        if self.has_events() {
            // Wake the executor so the next poll hands out the rest.
            rt.spawn(async |_| {}).detach();
        }
        ran
    }

    fn has_events(&self) -> bool {
        !self.store.events.is_empty() || !self.nav.events.is_empty() || !self.covers.asks.is_empty()
    }

    /// Hands every queued event to its listeners: the store's, then the
    /// navigator's, then the cover asks. False when none was queued.
    fn dispatch(&mut self) -> bool {
        if !self.has_events() {
            return false;
        }
        let store = std::mem::take(&mut self.store.events);
        let nav = std::mem::take(&mut self.nav.events);
        let covers = std::mem::take(&mut self.covers.asks);
        let listeners = self.store_listeners.clone();
        for event in &store {
            for f in &listeners {
                f(self, event);
            }
        }
        let listeners = self.nav_listeners.clone();
        for event in &nav {
            for f in &listeners {
                f(self, event);
            }
        }
        let listeners = self.cover_listeners.clone();
        for ask in &covers {
            for f in &listeners {
                f(self, ask);
            }
        }
        true
    }
}
