//! The desktop shell frame (`+layout.svelte` desk markup + CSS 1869-2840):
//! boot states, tab strip, the grid of rail | sidebar | main | right pane,
//! drawer tabs, resizers, overlays and global shortcuts. At a phone width
//! [`Workspace`] renders the mobile shell (`mobile.rs`) instead.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::IconNamed as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::Route;
use roostr_core::store::Store;
use roostr_core::types::Space;
use roostr_core::{
    Connection, Glyph, NavEvent, Navigator, PaneState, StoreEvent, UiState, motion, theme,
};
use web_time::Instant;

use crate::create_menu::CreateMenu;
use crate::header::{DeleteProp, MoreMenu};
use crate::modal::Prompt;
use crate::pinned::PinnedWidget;
use crate::search::SearchModal;
use crate::{NewObject, OpenSettings, PaneHost, Search};

/// Builds the view for a route's main area (provided by the app binary).
pub type PageFactory = Rc<dyn Fn(&Route, &mut Window, &mut App) -> AnyView>;
/// Builds the identity-gate view (see [`Workspace::identity_gate`]).
type GateBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;
/// Opens Settings (see [`Workspace::on_open_settings`]).
type SettingsHook = Rc<dyn Fn(&mut Window, &mut App)>;
/// Window bounds of each space card by space id, from the last frame.
pub(crate) type CardBounds = Rc<std::cell::RefCell<Vec<(String, Bounds<Pixels>)>>>;

pub(crate) const RAIL_MIN: f32 = 56.;
pub(crate) const RAIL_MAX: f32 = 320.;
const RAIL_WIDE_DEFAULT: f32 = 236.;
const SIDE_DEFAULT: f32 = 220.;
const SIDE_MIN: f32 = 180.;
const SIDE_MAX: f32 = 480.;
/// Resting on a drawer tab this long toggles it.
const DWELL: Duration = Duration::from_millis(700);
/// A drawer tab's hit area: its width with a mouse, and on a touch screen.
const DRAWER_TAB_W: f32 = 16.;
const DRAWER_TAB_TOUCH_W: f32 = 24.;

/// How far a drawer tab's hit area reaches past the 16px slot it is placed in.
fn drawer_tab_spill() -> f32 {
    if roostr_core::layout::is_touch() {
        (DRAWER_TAB_TOUCH_W - DRAWER_TAB_W) / 2.
    } else {
        0.
    }
}
/// `[data-tip]` appears after this much hover (`animation: tip-in 0.12s ease 0.4s`),
/// then fades in over [`motion::FADE`].
const TIP_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DwellTab {
    Nav,
    Pane,
}

struct Dwell {
    which: DwellTab,
    started: Instant,
    _timer: Task<()>,
}

pub(crate) struct RailDrag {
    start_x: Pixels,
    start_w: f32,
    moved: bool,
}

impl RailDrag {
    pub(crate) fn new(start_x: Pixels, start_w: f32) -> Self {
        Self {
            start_x,
            start_w,
            moved: false,
        }
    }
}

/// A grid column's width (`transition: grid-template-columns 0.16s ease`),
/// following the pointer 1:1 while a resizer drags (`transition: none`).
fn column(
    state: &mut motion::ValueState,
    target: f32,
    animate: bool,
    window: &mut Window,
    cx: &App,
) -> f32 {
    let transition = if animate {
        motion::COLUMNS.transition()
    } else {
        motion::Transition::Instant
    };
    state
        .frame(f64::from(target), transition, window, cx)
        .value() as f32
}

/// The shell. Build it once per window with the page factory; it follows
/// the global [`Navigator`], [`Store`] and [`PaneState`].
pub struct Workspace {
    pages: PageFactory,
    page_cache: HashMap<Route, AnyView>,
    pub(crate) scrolls: HashMap<Route, ScrollHandle>,
    pub(crate) focus: FocusHandle,
    open_settings: Option<SettingsHook>,
    identity_gate: Option<GateBuilder>,
    gate_view: Option<AnyView>,
    // Layout (persisted per device like the web's localStorage keys).
    pub(crate) rail_width: f32,
    rail_wide_width: f32,
    pub(crate) rail_drag: Option<RailDrag>,
    pub(crate) side_width: f32,
    side_drag: Option<(Pixels, f32)>,
    pane_drag: Option<(Pixels, f32)>,
    pub(crate) nav_hidden: bool,
    dwell: Option<Dwell>,
    rail_col: motion::ValueState,
    side_col: motion::ValueState,
    pane_col: motion::ValueState,
    /// The desk has rendered: columns animate only after its first frame
    /// (a CSS transition doesn't run on mount).
    desk_shown: bool,
    // Sidebar.
    /// Folds this device asked for that the pinned objects don't show yet.
    pub(crate) pin_folds: HashMap<String, crate::pin_collapse::PendingFold>,
    /// The legacy device-local `widget-collapsed` folds still to migrate.
    pub(crate) legacy_folds: HashMap<String, bool>,
    pub(crate) section_collapsed: HashMap<String, bool>,
    pub(crate) pinned_views: HashMap<String, Entity<PinnedWidget>>,
    pub(crate) create_menu: Option<Entity<CreateMenu>>,
    pub(crate) new_prop: Option<Entity<roostr_properties::PropertySuggest>>,
    pub(crate) sidebar_scroll: ScrollHandle,
    // Drag reorders: (dragged, (over, after/bottom)).
    pub(crate) space_drag: Option<String>,
    pub(crate) space_over: Option<(String, bool)>,
    pub(crate) tab_drag: Option<u64>,
    pub(crate) tab_over: Option<(u64, bool)>,
    pub(crate) widget_drag: Option<String>,
    pub(crate) widget_over: Option<(String, bool)>,
    // Header.
    pub(crate) more: Option<MoreMenu>,
    pub(crate) delete_blocker: (String, String),
    pub(crate) delete_prop: Option<DeleteProp>,
    // Overlays.
    pub(crate) search: Option<Entity<SearchModal>>,
    pub(crate) prompt: Option<Entity<Prompt>>,
    space_order_moved: bool,
    /// A Personal space is being created for an empty vault.
    seeding_personal: bool,
    /// Window bounds of the dropdown anchors (New pill, More, Properties ＋), from the last frame.
    anchors: Rc<std::cell::RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    /// The hovered `[data-tip]` control: (anchor key, text, past the delay).
    tip: Option<(&'static str, SharedString, bool)>,
    _tip_timer: Option<Task<()>>,
    /// The `[data-tip]` controls drawn by the last frame that drew this view.
    tips_drawn: Rc<std::cell::RefCell<std::collections::HashSet<&'static str>>>,
    /// Keyboard focus handles of the tabs, by uid.
    pub(crate) tab_focus: HashMap<u64, FocusHandle>,
    // Mobile shell (`isMobile`).
    /// A space card was tapped open: its home shows instead of the Spaces list.
    pub(crate) mobile_space_open: bool,
    /// `mCollapsed`: folded sections of the space home (pinned cards fold on the object).
    pub(crate) mobile_collapsed: HashMap<String, bool>,
    /// The space card held up for a reorder (`spaceLiftId`).
    pub(crate) space_lift: Option<String>,
    /// Window bounds of the space cards, from the last frame (what a lifted card is over).
    pub(crate) card_bounds: CardBounds,
    /// A pull-to-sync gesture in flight: how far it has pulled at the top.
    pub(crate) pull: Rc<std::cell::Cell<Option<f32>>>,
    pub(crate) mobile_spaces_scroll: ScrollHandle,
    pub(crate) mobile_home_scroll: ScrollHandle,
    /// The phone's push/pop slide and back swipe.
    pub(crate) back: crate::back_swipe::BackSwipe,
    /// The route shown before the latest navigation event (`here` when the space changes).
    pub(crate) shown_route: Route,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// The shell around `pages` (the route → main-area view factory).
    pub fn new(pages: PageFactory, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let nav = Navigator::global(cx);
        let pane = PaneState::global(cx);
        let host = PaneHost::global(cx);
        let subscriptions = vec![
            cx.observe(&store, |this, _, cx| {
                this.on_store_changed(cx);
                cx.notify();
            }),
            cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                if matches!(event, StoreEvent::Refreshed) {
                    this.refresh_delete_blocker(true, cx);
                }
            }),
            cx.observe(&nav, |_, _, cx| cx.notify()),
            cx.subscribe(&nav, |this, _, event: &NavEvent, cx| {
                this.on_navigated(event, cx)
            }),
            cx.subscribe_in(&nav, window, |this, _, event: &NavEvent, window, cx| {
                if *event == NavEvent::SpaceChanged {
                    this.mobile_space_changed(window, cx);
                }
            }),
            cx.observe(&pane, |_, _, cx| cx.notify()),
            cx.observe(&host, |_, _, cx| cx.notify()),
            cx.observe(
                &roostr_core::profile::ProfileCache::global(cx),
                |_, _, cx| cx.notify(),
            ),
        ];
        let rail_width = UiState::get::<serde_json::Value>(cx, "rail-width")
            .and_then(|v| json_number(&v))
            .filter(|w| *w > 0.)
            .unwrap_or_else(|| {
                if UiState::get::<serde_json::Value>(cx, "rail-wide")
                    .and_then(|v| v.as_str().map(|s| s == "1"))
                    == Some(true)
                {
                    RAIL_WIDE_DEFAULT
                } else {
                    RAIL_MIN
                }
            })
            .clamp(RAIL_MIN, RAIL_MAX);
        let rail_wide_width = UiState::get::<serde_json::Value>(cx, "rail-width-wide")
            .and_then(|v| json_number(&v))
            .filter(|w| *w > 0.)
            .unwrap_or(RAIL_WIDE_DEFAULT);
        let side_width = UiState::get::<serde_json::Value>(cx, "side-width")
            .and_then(|v| json_number(&v))
            .filter(|w| *w > 0.)
            .unwrap_or(SIDE_DEFAULT);
        let nav_hidden = UiState::get::<serde_json::Value>(cx, "nav-hidden")
            .is_some_and(|v| v.as_str() == Some("1") || v.as_bool() == Some(true));
        let legacy_folds = crate::pin_collapse::legacy_folds(cx);
        let section_collapsed = UiState::get(cx, "section-collapsed").unwrap_or_default();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let (rail_col, side_col) = if nav_hidden {
            (0., 0.)
        } else {
            (rail_width, side_width)
        };
        let mut this = Self {
            pages,
            page_cache: HashMap::new(),
            scrolls: HashMap::new(),
            focus,
            open_settings: None,
            identity_gate: None,
            gate_view: None,
            rail_width,
            rail_wide_width,
            rail_drag: None,
            side_width,
            side_drag: None,
            pane_drag: None,
            nav_hidden,
            dwell: None,
            rail_col: motion::ValueState::new(f64::from(rail_col)),
            side_col: motion::ValueState::new(f64::from(side_col)),
            pane_col: motion::ValueState::new(0.),
            desk_shown: false,
            pin_folds: HashMap::new(),
            legacy_folds,
            section_collapsed,
            pinned_views: HashMap::new(),
            create_menu: None,
            new_prop: None,
            sidebar_scroll: ScrollHandle::new(),
            space_drag: None,
            space_over: None,
            tab_drag: None,
            tab_over: None,
            widget_drag: None,
            widget_over: None,
            more: None,
            delete_blocker: (String::new(), String::new()),
            delete_prop: None,
            search: None,
            prompt: None,
            space_order_moved: false,
            seeding_personal: false,
            anchors: Rc::default(),
            tip: None,
            _tip_timer: None,
            tips_drawn: Rc::default(),
            tab_focus: HashMap::new(),
            mobile_space_open: false,
            mobile_collapsed: HashMap::new(),
            space_lift: None,
            card_bounds: Rc::default(),
            pull: Rc::default(),
            back: Default::default(),
            mobile_spaces_scroll: ScrollHandle::new(),
            mobile_home_scroll: ScrollHandle::new(),
            shown_route: nav.read(cx).route().clone(),
            _subscriptions: subscriptions,
        };
        this.on_store_changed(cx);
        this
    }

    /// What the rail's account button (and [`OpenSettings`]) opens: the app
    /// passes roostr-settings' `open_settings`.
    pub fn on_open_settings(&mut self, open: impl Fn(&mut Window, &mut App) + 'static) {
        self.open_settings = Some(Rc::new(open));
    }

    /// The view shown instead of the workspace while the daemon has no
    /// identity (`hasKey == false`); built once, when first needed.
    pub fn identity_gate(&mut self, gate: impl Fn(&mut Window, &mut App) -> AnyView + 'static) {
        self.identity_gate = Some(Rc::new(gate));
        self.gate_view = None;
    }

    // ------------------------------------------------------------ state

    /// The space whose sidebar shows (`current`): the active space, else the first.
    pub(crate) fn current_space(cx: &App) -> Option<Space> {
        let store = Store::global(cx).read(cx);
        let active = Navigator::global(cx).read(cx).space().to_string();
        store
            .space(&active)
            .or_else(|| store.channels.first())
            .cloned()
    }

    /// `activeSpace.id || defaultChannelId`.
    pub(crate) fn space_id(cx: &App) -> String {
        let active = Navigator::global(cx).read(cx).space().to_string();
        if !active.is_empty() {
            return active;
        }
        Store::global(cx)
            .read(cx)
            .channels
            .first()
            .map(|c| c.id.clone())
            .unwrap_or_default()
    }

    /// The object open in the main area, if any (`objectId`).
    pub(crate) fn open_object_id(cx: &App) -> Option<String> {
        match Navigator::global(cx).read(cx).route() {
            Route::Object(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn on_store_changed(&mut self, cx: &mut Context<Self>) {
        let (loaded, has_channels, first, ready, status) = {
            let store = Store::global(cx).read(cx);
            (
                store.loaded,
                !store.channels.is_empty(),
                store.channels.first().map(|c| c.id.clone()),
                store.connection == Connection::Ready,
                store.status.clone(),
            )
        };
        if !ready || !loaded {
            return;
        }
        // Boot: the remembered space if it still exists, else the first. A
        // vault still being prepared may not show the remembered one yet.
        let nav = Navigator::global(cx);
        if nav.read(cx).space().is_empty() && has_channels {
            let remembered = Navigator::remembered_space(cx);
            let found = remembered
                .clone()
                .filter(|id| Store::global(cx).read(cx).space(id).is_some());
            let wait = remembered.is_some() && found.is_none() && status.preparing.is_some();
            if let Some(id) = found.or(first).filter(|_| !wait) {
                nav.update(cx, |nav, cx| nav.use_space(&id, cx));
            }
        }
        if has_channels {
            self.seeding_personal = false;
        } else if !self.seeding_personal
            && status.phase == roostr_core::types::SyncPhase::Live
            && status.bootstrapped
        {
            self.seed_personal(cx);
        }
        if !self.space_order_moved && status.preparing.is_none() {
            self.space_order_moved = true;
            self.move_space_order(cx);
        }
        self.migrate_pin_folds(cx);
        // Pinned widgets of spaces no longer pinned are dropped.
        if let Some(space) = Self::current_space(cx) {
            self.pinned_views
                .retain(|id, _| space.pinned_ids.contains(id));
        }
    }

    /// A fresh key's empty vault gets a Personal space - but only once a
    /// complete history walk proved it empty (`bootstrapped`, which only a
    /// replica reports), never mid-backfill. Stays guarded until the space
    /// shows up, so the refreshes its changes cause never fork a second one.
    fn seed_personal(&mut self, cx: &mut Context<Self>) {
        self.seeding_personal = true;
        let task = roostr_core::api::space::create("Personal", None, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(id) => {
                    Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
                    Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&id, cx));
                }
                Err(error) => {
                    log::warn!("could not create the Personal space: {error:#}");
                    this.seeding_personal = false;
                }
            });
        })
        .detach();
    }

    fn on_navigated(&mut self, event: &NavEvent, cx: &mut Context<Self>) {
        if *event != NavEvent::Navigated {
            return;
        }
        let nav = Navigator::global(cx);
        let route = nav.read(cx).route().clone();
        // While the shell waits for the remembered space, the sidebar shows
        // the first space ([`Self::current_space`]): a page opened from it
        // was opened in that space, so that space is the one picked - the
        // remembered one arriving later must not swap its saved tabs in.
        if nav.read(cx).space().is_empty()
            && route.is_workspace_page()
            && let Some(shown) = Self::current_space(cx)
        {
            nav.update(cx, |nav, cx| nav.use_space(&shown.id, cx));
            return;
        }
        // A page that exists already publishes its own pane on Navigated;
        // anything else starts without one.
        let publishes = matches!(route, Route::Object(_)) && self.page_cache.contains_key(&route);
        if !publishes {
            PaneState::global(cx).update(cx, |pane, cx| pane.publish(false, 0, 0, cx));
            crate::set_pane(None, cx);
        }
        // Views live while a tab of this space shows their route.
        let live: Vec<Route> = nav
            .read(cx)
            .tabs()
            .iter()
            .map(|t| t.route.clone())
            .collect();
        self.page_cache.retain(|r, _| live.contains(r));
        self.scrolls.retain(|r, _| live.contains(r));
        self.more = None;
        self.shown_route = route;
        self.refresh_delete_blocker(false, cx);
        cx.notify();
    }

    /// `moveSpaceOrder`: the legacy `order` field on owned spaces moves into
    /// the preferences' space order, once per load.
    fn move_space_order(&mut self, cx: &mut Context<Self>) {
        let store = Store::global(cx);
        let owned: Vec<String> = store
            .read(cx)
            .channels
            .iter()
            .filter(|c| c.owner.is_empty())
            .map(|c| c.id.clone())
            .collect();
        let fetches: Vec<_> = owned
            .iter()
            .map(|id| (id.clone(), store.read(cx).fetch_object(id, cx)))
            .collect();
        cx.spawn(async move |_, cx| {
            let mut found: Vec<(String, f64)> = Vec::new();
            for (id, fetch) in fetches {
                if let Ok(object) = fetch.await
                    && let Some(n) = object.field("order").and_then(|v| v.as_number())
                {
                    found.push((id, n));
                }
            }
            if found.is_empty() {
                return;
            }
            cx.update(|cx| {
                let store = Store::global(cx);
                let missing: Vec<(String, f64)> = found
                    .iter()
                    .filter(|(id, _)| !store.read(cx).space_order.contains_key(id))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    let order = store.update(cx, |store, cx| {
                        for (id, n) in missing {
                            store.space_order.insert(id, n);
                        }
                        cx.notify();
                        store.space_order.clone()
                    });
                    roostr_core::api::user_prefs::set_space_order(&order, cx)
                        .detach_and_log_err(cx);
                }
                for (id, _) in &found {
                    roostr_core::api::note::delete_field(id, "order", cx).detach_and_log_err(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn refresh_delete_blocker(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(id) = Self::open_object_id(cx) else {
            self.delete_blocker = (String::new(), String::new());
            return;
        };
        let is_def = {
            let store = Store::global(cx).read(cx);
            store.relations.iter().any(|r| r.id == id)
                || store
                    .summary(&id)
                    .is_some_and(|s| s.type_key == "type" || s.type_key == "tool")
        };
        if !force && self.delete_blocker.0 == id {
            return;
        }
        self.delete_blocker = (id.clone(), String::new());
        if !is_def {
            return;
        }
        let fetch = Store::global(cx).read(cx).fetch_object(&id, cx);
        cx.spawn(async move |this, cx| {
            let Ok(object) = fetch.await else { return };
            let task = cx.update(|cx| roostr_core::protected::deletion_blocker(&object, cx));
            let why = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.delete_blocker.0 == id {
                    this.delete_blocker.1 = why;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---------------------------------------------------------- actions

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            return;
        }
        let modal = cx.new(|cx| SearchModal::new(window, cx));
        cx.subscribe(&modal, |this, _, _: &crate::search::Close, cx| {
            this.search = None;
            cx.notify();
        })
        .detach();
        self.search = Some(modal);
        cx.notify();
    }

    pub(crate) fn settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.open_settings.clone() {
            open(window, cx);
        }
    }

    pub(crate) fn toggle_nav(&mut self, cx: &mut Context<Self>) {
        self.nav_hidden = !self.nav_hidden;
        UiState::set(cx, "nav-hidden", &if self.nav_hidden { "1" } else { "0" });
        cx.notify();
    }

    fn toggle_pane(cx: &mut App) {
        PaneState::global(cx).update(cx, |pane, cx| {
            let open = !pane.open;
            pane.set_open(open, cx);
        });
    }

    fn dwell_start(&mut self, which: DwellTab, cx: &mut Context<Self>) {
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DWELL).await;
            let _ = this.update(cx, |this, cx| {
                if this.dwell.as_ref().is_some_and(|d| d.which == which) {
                    this.dwell = None;
                    match which {
                        DwellTab::Nav => this.toggle_nav(cx),
                        DwellTab::Pane => Self::toggle_pane(cx),
                    }
                    cx.notify();
                }
            });
        });
        self.dwell = Some(Dwell {
            which,
            started: Instant::now(),
            _timer: timer,
        });
        cx.notify();
    }

    fn dwell_cancel(&mut self, cx: &mut Context<Self>) {
        if self.dwell.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn rail_persist(&self, cx: &mut App) {
        UiState::set(cx, "rail-width", &self.rail_width.round().to_string());
        if self.rail_width > 100. {
            UiState::set(cx, "rail-width-wide", &self.rail_width.round().to_string());
        }
    }

    pub(crate) fn rail_toggle(&mut self, cx: &mut Context<Self>) {
        self.rail_width = if self.rail_width > 100. {
            RAIL_MIN
        } else {
            self.rail_wide_width.max(160.)
        };
        self.rail_persist(cx);
        cx.notify();
    }

    fn on_resize_move(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.rail_drag {
            let dx = f32::from(position.x - drag.start_x);
            if dx.abs() > 4. {
                drag.moved = true;
            }
            self.rail_width = (drag.start_w + dx).clamp(RAIL_MIN, RAIL_MAX);
            cx.notify();
        } else if let Some((start_x, start_w)) = self.side_drag {
            self.side_width = (start_w + f32::from(position.x - start_x)).clamp(SIDE_MIN, SIDE_MAX);
            cx.notify();
        } else if let Some((start_x, start_w)) = self.pane_drag {
            let width = (start_w + f32::from(start_x - position.x))
                .clamp(roostr_core::pane::MIN_WIDTH, roostr_core::pane::MAX_WIDTH);
            let _ = window;
            PaneState::global(cx).update(cx, |pane, cx| {
                pane.width = width;
                cx.notify();
            });
        }
    }

    fn on_resize_end(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.rail_drag.take() {
            if !drag.moved {
                self.rail_toggle(cx);
                return;
            }
            if self.rail_width < 140. {
                self.rail_width = RAIL_MIN;
            } else {
                self.rail_wide_width = self.rail_width;
            }
            self.rail_persist(cx);
        } else if self.side_drag.take().is_some() {
            UiState::set(cx, "side-width", &self.side_width.round().to_string());
        } else if self.pane_drag.take().is_some() {
            let width = PaneState::global(cx).read(cx).width;
            PaneState::global(cx).update(cx, |pane, cx| {
                pane.width = 0.;
                pane.set_width(width, cx);
            });
        }
        cx.notify();
    }

    fn resizing(&self) -> bool {
        self.rail_drag.is_some() || self.side_drag.is_some() || self.pane_drag.is_some()
    }

    /// Close every dropdown behind the click-away backdrop.
    pub(crate) fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.more = None;
        self.create_menu = None;
        cx.notify();
    }

    // ---------------------------------------------------------- rendering

    pub(crate) fn page(
        &mut self,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyView {
        if let Some(view) = self.page_cache.get(route) {
            return view.clone();
        }
        let view = (self.pages)(route, window, cx);
        self.page_cache.insert(route.clone(), view.clone());
        view
    }

    fn render_boot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (connection, has_key, progress) = {
            let store = Store::global(cx).read(cx);
            (
                store.connection.clone(),
                store.settings.as_ref().map(|s| s.has_key),
                roostr_core::sync_status::boot_progress(&store.status),
            )
        };
        let status = |text: SharedString, color: Hsla| {
            div()
                .w_full()
                .flex()
                .justify_center()
                .my(px(16.))
                .text_color(color)
                .child(text)
        };
        let frame = || {
            div()
                .id("boot")
                .size_full()
                .flex()
                .flex_col()
                .bg(theme::bg())
                .text_color(theme::fg())
                .text_size(px(16.))
                .font_family(theme::fonts::UI)
        };
        match connection {
            Connection::Ready => None,
            Connection::Connecting => Some(
                frame()
                    .child(status("Opening your vault…".into(), theme::fg()))
                    .children(progress.map(|line| status(line.into(), theme::muted())))
                    .into_any_element(),
            ),
            Connection::Failed(error) => {
                if has_key == Some(false)
                    && let Some(gate) = self.identity_gate.clone()
                {
                    let view = self
                        .gate_view
                        .get_or_insert_with(|| gate(window, cx))
                        .clone();
                    return Some(frame().child(view).into_any_element());
                }
                Some(
                    frame()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .p(px(48.))
                                .gap(px(16.))
                                .child("Your identity and local changes remain on this device.")
                                .child(
                                    crate::modal::outline_button(
                                        "retry",
                                        "Retry opening vault",
                                        false,
                                    )
                                    .on_click(|_, _, cx| {
                                        Store::global(cx)
                                            .update(cx, |store, cx| store.reconnect(cx));
                                    }),
                                ),
                        )
                        .child(status(error, theme::fg()))
                        .into_any_element(),
                )
            }
        }
    }

    fn render_drawer_tab(
        &self,
        which: DwellTab,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let dwelling = self
            .dwell
            .as_ref()
            .filter(|d| d.which == which)
            .map(|d| d.started);
        let fill =
            dwelling.map(|started| (started.elapsed().as_secs_f32() / DWELL.as_secs_f32()).min(1.));
        let tip = match (which, open) {
            (DwellTab::Nav, true) => "Hide sidebar",
            (DwellTab::Nav, false) => "Show sidebar",
            (DwellTab::Pane, true) => "Hide chat & properties",
            (DwellTab::Pane, false) => "Show chat & properties",
        };
        let id = match which {
            DwellTab::Nav => "nav-drawer",
            DwellTab::Pane => "pane-drawer",
        };
        let hover_key = match which {
            DwellTab::Nav => "nav-drawer-hover",
            DwellTab::Pane => "pane-drawer-hover",
        };
        // A slim pill on an invisible hit area: a hairline at rest that
        // thickens and darkens on hover (`transition: 0.12s ease`). The hit
        // area is wider and taller on a touch screen.
        let (hover, hover_state) = motion::hover(hover_key, motion::FADE, window, cx);
        let touch = roostr_core::layout::is_touch();
        let (hit_w, hit_h) = if touch {
            (DRAWER_TAB_TOUCH_W, 96.)
        } else {
            (DRAWER_TAB_W, 72.)
        };
        let pill_h = 32. + 8. * hover;
        let pill = div()
            .relative()
            .w(px(3. + 1. * hover))
            .h(px(pill_h))
            .rounded_full()
            .overflow_hidden()
            .bg(motion::mix(theme::border(), theme::muted(), hover))
            .when_some(fill, |d, fill| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(relative(fill))
                        .bg(theme::accent()),
                )
            });
        div()
            .id(id)
            .absolute()
            .top(relative(0.5))
            .mt(px(-hit_h / 2.))
            .w(px(hit_w))
            .h(px(hit_h))
            .flex()
            .items_center()
            // Shut, the pill sits a few px in from the window edge.
            .map(|d| match (which, open) {
                (_, true) => d.justify_center(),
                (DwellTab::Nav, false) => d.justify_start().pl(px(4.)),
                (DwellTab::Pane, false) => d.justify_end().pr(px(4.)),
            })
            .cursor_pointer()
            .child(pill)
            .tooltip(crate::tip(tip))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                motion::HoverMotion::set(&hover_state, *hovered, cx);
                if *hovered {
                    this.dwell_start(which, cx);
                } else {
                    this.dwell_cancel(cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dwell_cancel(cx);
                match which {
                    DwellTab::Nav => this.toggle_nav(cx),
                    DwellTab::Pane => Self::toggle_pane(cx),
                }
            }))
    }

    fn render_main(&mut self, wide: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let route = Navigator::global(cx).read(cx).route().clone();
        let page = self.page(&route, window, cx);
        let scroll = self.scrolls.entry(route.clone()).or_default().clone();
        div()
            .flex_1()
            .min_w_0()
            .m(px(6.))
            .rounded(theme::radius::page())
            .bg(theme::bg())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .px(px(32.))
                    .child(self.render_header(wide, window, cx)),
            )
            .child(
                div()
                    .id(("main-scroll", page.entity_id()))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px(px(32.))
                    .child(
                        div()
                            .w_full()
                            .pt(px(24.))
                            .pb(px(80.))
                            .mx_auto()
                            .map(|d| {
                                if wide {
                                    d.px(px(32.))
                                } else {
                                    d.max_w(px(920.))
                                }
                            })
                            .child(page),
                    ),
            )
    }

    fn render_desk(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Build (or fetch) the page first: a new page publishes its pane while being built.
        let route = Navigator::global(cx).read(cx).route().clone();
        self.page(&route, window, cx);
        let pane = PaneState::global(cx).read(cx);
        let (pane_open, pane_available, pane_width) = (pane.open, pane.available, pane.width);
        let pane_view = PaneHost::global(cx).read(cx).view.clone();
        let affixed = pane_open && pane_available && pane_view.is_some();
        let animate = !self.resizing() && self.desk_shown;
        self.desk_shown = true;
        let rail_target = if self.nav_hidden { 0. } else { self.rail_width };
        let rail_col = column(&mut self.rail_col, rail_target, animate, window, cx);
        let side_target = if self.nav_hidden { 0. } else { self.side_width };
        let side_col = column(&mut self.side_col, side_target, animate, window, cx);
        let pane_target = if affixed { pane_width } else { 0. };
        let pane_col = column(&mut self.pane_col, pane_target, animate, window, cx);
        let wide = Self::open_object_id(cx)
            .and_then(|id| {
                Store::global(cx)
                    .read(cx)
                    .summary(&id)
                    .map(|s| s.type_key.clone())
            })
            .is_some_and(|k| matches!(k.as_str(), "query" | "set" | "collection"));
        let tabs_shown = Navigator::global(cx).read(cx).tabs().len() > 1;
        let nav_tab_left = if self.nav_hidden {
            0.
        } else {
            rail_col + side_col + 3. - 8. - drawer_tab_spill()
        };
        // The drawer dwell fill grows frame by frame.
        if self.dwell.is_some() {
            window.request_animation_frame();
        }

        let shell = div()
            .id("shell")
            .debug_selector(|| "desk".into())
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(theme::gutter())
            // Rail column.
            .child(
                div()
                    .flex_none()
                    .w(px(rail_col))
                    .h_full()
                    .py(px(6.))
                    .when(!self.nav_hidden && rail_col > 6., |d| {
                        d.child(self.render_rail(rail_col - 6., window, cx))
                    }),
            )
            // Sidebar column.
            .child(
                div()
                    .flex_none()
                    .w(px(side_col))
                    .h_full()
                    .py(px(6.))
                    .when(!self.nav_hidden && side_col > 6., |d| {
                        d.child(self.render_sidebar(side_col - 6., window, cx))
                    }),
            )
            .child(self.render_main(wide, window, cx))
            // Right pane column.
            .child(
                div()
                    .flex_none()
                    .w(px(pane_col))
                    .h_full()
                    .py(px(6.))
                    .when_some(pane_view.filter(|_| pane_col > 6.), |d, view| {
                        d.child(
                            div()
                                .flex_none()
                                .w(px(pane_col - 6.))
                                .h_full()
                                .rounded(theme::radius::page())
                                .bg(theme::bg())
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .child(view),
                        )
                    }),
            )
            // Resizers (in the gaps between panes).
            .when(!self.nav_hidden, |d| {
                d.child(self.render_rail_resize(rail_col, window, cx))
                    .child(
                        div()
                            .id("side-resize")
                            .absolute()
                            .top(px(6.))
                            .bottom(px(6.))
                            .left(px(rail_col + side_col - 8.))
                            .w(px(8.))
                            .cursor_col_resize()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                    this.side_drag = Some((e.position.x, this.side_width));
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    )
            })
            .when(affixed, |d| {
                d.child(
                    div()
                        .id("dd-resize")
                        .absolute()
                        .top(px(6.))
                        .bottom(px(6.))
                        .right(px(pane_col - 6.))
                        .w(px(12.))
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                this.pane_drag =
                                    Some((e.position.x, PaneState::global(cx).read(cx).width));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
            })
            // Drawer tabs.
            .child(
                self.render_drawer_tab(DwellTab::Nav, !self.nav_hidden, window, cx)
                    .left(px(nav_tab_left)),
            )
            .when(pane_available, |d| {
                let right = if pane_open {
                    (pane_col + 3. - 8. - drawer_tab_spill()).max(0.)
                } else {
                    0.
                };
                d.child(
                    self.render_drawer_tab(DwellTab::Pane, pane_open, window, cx)
                        .right(px(right)),
                )
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .when(tabs_shown, |d| d.child(self.render_tab_strip(cx)))
            .child(shell)
            .into_any_element()
    }

    /// Overlays are plain last children of the root (not `deferred`): a
    /// deferred subtree does not join the focus path, so its inputs would get
    /// typed text but never the arrow/Enter/Escape actions.
    fn render_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        // The mobile shell has no click-away backdrop: its ⋯ and ＋ toggle.
        let mobile = roostr_core::layout::is_mobile(window);
        if !mobile && (self.more.is_some() || self.create_menu.is_some()) {
            out.push(
                div()
                    .id("menu-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_menus(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element(),
            );
        }
        // Dropdowns drop in (`motion::appear`) and close at once.
        let anchors = self.anchors.borrow().clone();
        if mobile {
            out.extend(self.render_m_create_menu(window, cx));
        } else if let Some(menu) = self.create_menu.clone()
            && let Some(b) = anchors.get("create")
        {
            let max_h = window.viewport_size().height - px(110.);
            let enter = motion::appear_frame("create-menu-appear", -4., window, cx);
            out.push(
                anchored()
                    .position(point(b.right(), b.top() + px(32.)))
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(enter.apply(div().max_h(max_h).flex().child(menu)))
                    .into_any_element(),
            );
        }
        if self.more.is_some()
            && let Some(b) = anchors.get("more")
        {
            let menu = self.render_more_menu(window, cx);
            let enter = motion::appear_frame("more-menu-appear", -4., window, cx);
            out.push(
                anchored()
                    .position(point(b.right(), b.top() + px(32.)))
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(enter.apply(menu))
                    .into_any_element(),
            );
        }
        if let Some(suggest) = self.new_prop.clone()
            && let Some(b) = anchors.get("new-prop")
        {
            let enter = motion::appear_frame("new-prop-appear", -4., window, cx);
            out.push(
                anchored()
                    .position(point(b.left(), b.bottom() + px(4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(enter.apply(div().child(suggest)))
                    .into_any_element(),
            );
        }
        if self.delete_prop.is_some() {
            out.push(self.render_delete_prop(window, cx));
        }
        if let Some((key, text, true)) = self.tip.clone()
            && let Some(b) = anchors.get(key)
        {
            let fade = motion::appear_frame(key, 0., window, cx);
            out.push(
                anchored()
                    .position(point(b.center().x, b.bottom() + px(6.)))
                    .anchor(Anchor::TopCenter)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        fade.apply(div())
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(theme::radius::sm())
                            .px(px(8.))
                            .py(px(3.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::NORMAL)
                            .line_height(relative(theme::LINE_HEIGHT_NORMAL))
                            .text_color(theme::fg())
                            .whitespace_nowrap()
                            .shadow(roostr_core::ui::tooltip_shadow())
                            .child(text),
                    )
                    .into_any_element(),
            );
        }
        if let Some(search) = &self.search {
            out.push(search.clone().into_any_element());
        }
        if let Some(prompt) = &self.prompt {
            out.push(prompt.clone().into_any_element());
        }
        out
    }

    /// `[data-tip]`: a pill centred 6px below the control, after 400ms
    /// hover, fading in over 120ms. Like CSS's `:hover::after`, it goes with
    /// the control (see [`Self::drop_orphan_tip`]).
    pub(crate) fn data_tip(
        &self,
        el: Stateful<Div>,
        key: &'static str,
        text: &'static str,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let anchors = self.anchors.clone();
        let drawn = self.tips_drawn.clone();
        el.relative()
            .child(probe(move |bounds| {
                anchors.borrow_mut().insert(key, bounds);
                drawn.borrow_mut().insert(key);
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.tip = Some((key, text.into(), false));
                    this._tip_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(TIP_DELAY).await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(tip) = this.tip.as_mut().filter(|t| t.0 == key) {
                                tip.2 = true;
                                cx.notify();
                            }
                        });
                    }));
                } else if this.tip.as_ref().is_some_and(|t| t.0 == key) {
                    this.tip = None;
                    this._tip_timer = None;
                }
                cx.notify();
            }))
    }

    /// Forget the tip of a control the last frame no longer drew. A control
    /// taken off screen under the pointer (the back button a tap navigated
    /// away from) never hears its hover end, so its tip would come up after
    /// the delay and stay, at the control's old place. Runs at the start of
    /// each render, before this frame's controls are drawn.
    fn drop_orphan_tip(&mut self) {
        let drawn = std::mem::take(&mut *self.tips_drawn.borrow_mut());
        if self
            .tip
            .as_ref()
            .is_some_and(|(key, ..)| !drawn.contains(key))
        {
            self.tip = None;
            self._tip_timer = None;
        }
    }

    /// An invisible probe recording its parent's window bounds under `key`
    /// (where a dropdown anchors).
    pub(crate) fn anchor_probe(&self, key: &'static str) -> impl IntoElement {
        let anchors = self.anchors.clone();
        probe(move |bounds| {
            anchors.borrow_mut().insert(key, bounds);
        })
    }
}

/// An invisible element covering its parent that hands `record` its window
/// bounds each time it is drawn.
fn probe(record: impl FnOnce(Bounds<Pixels>) + 'static) -> impl IntoElement {
    canvas(move |bounds, _, _| record(bounds), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// The ▶ disclosure arrow at `turn` (0 closed, 1 open: rotated 90°).
pub(crate) fn arrow_at(turn: f32, size: f32) -> Svg {
    svg()
        .path(Glyph::Disclosure.path())
        .size(px(size))
        .flex_none()
        .with_transformation(Transformation::rotate(radians(
            turn * std::f32::consts::FRAC_PI_2,
        )))
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drop_orphan_tip();
        let root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(theme::gutter())
            .text_color(theme::fg())
            .font_family(theme::fonts::UI)
            .text_size(px(16.))
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_search(window, cx)))
            .on_action(|_: &crate::FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &crate::FocusPrev, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.settings(window, cx)))
            .on_action(
                cx.listener(|this, _: &NewObject, window, cx| this.open_create_menu(window, cx)),
            );
        if let Some(boot) = self.render_boot(window, cx) {
            return root.child(boot);
        }
        // Full-window routes: the website pages and the full-screen chat.
        let route = Navigator::global(cx).read(cx).route().clone();
        if !matches!(route, Route::Site(_)) && roostr_core::layout::is_mobile(window) {
            return root
                .child(self.render_mobile(window, cx))
                .children(self.render_overlays(window, cx));
        }
        if matches!(route, Route::Site(_) | Route::Chat(_)) {
            let page = self.page(&route, window, cx);
            return root.child(page).children(self.render_overlays(window, cx));
        }
        let resizing = self.resizing();
        root.child(self.render_desk(window, cx))
            .children(self.render_overlays(window, cx))
            .when(resizing, |d| {
                d.cursor_col_resize().child(
                    canvas(|_, _, _| {}, {
                        let this = cx.entity().downgrade();
                        move |_, _, window, _| {
                            let move_this = this.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ = move_this.update(cx, |this, cx| {
                                        this.on_resize_move(e.position, window, cx)
                                    });
                                }
                            });
                            let up_this = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ = up_this.update(cx, |this, cx| this.on_resize_end(cx));
                                }
                            });
                        }
                    })
                    .absolute()
                    .size_full(),
                )
            })
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A persisted number stored as a JSON number or a numeric string (the web stores strings).
fn json_number(v: &serde_json::Value) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f32>().ok()))
}
