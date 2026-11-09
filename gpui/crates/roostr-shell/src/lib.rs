//! App shell (`src/routes/app/+layout.svelte`): space rail, sidebar (space
//! head, New menu, pinned widgets, Recently edited / Types / Properties),
//! per-space tab strip, page header with back/forward, search path, graph link
//! and the More menu, drawer tabs, resizers, the search modal (⌘K) and the
//! boot states. The main area shows what [`Pages::page`] draws for the active
//! route; the right "Chat & properties" columns show [`Pages::pane`] while the
//! page publishes a pane (`s.pane.publish`). At a phone width
//! ([`is_mobile`]) the mobile shell replaces all of that (Spaces list → space
//! home → object).
//!
//! The app calls [`register`] once with its [`Pages`], opens a window with
//! [`view`], [`CHROME`] and [`on_key`], and calls [`search`], [`new_object`]
//! and [`open_settings`] from its menus.

mod back_swipe;
mod create_menu;
mod drag;
mod header;
mod list_motion;
mod mobile;
mod modal;
mod pin_collapse;
mod pinned;
mod rail;
mod search;
mod shell;
mod sidebar;
mod tabs;
mod workspace;

use roostr_core::nav::Route;
use roostr_core::{PaneTab, State};
use roostr_ui::Theme;
use rune::{Chord, Cx, El, KeyEvent, Tx};

pub use shell::{RAIL_MAX, RAIL_MIN, SIDE_DEFAULT, SIDE_MAX, SIDE_MIN};

/// Draws a route's main area: `(state, cx, route, overlay layer)`.
pub type PageFn = fn(&State, &mut Cx<State>, &Route, &mut Vec<El>) -> El;
/// Draws a part of the open object's Chat & Properties pane.
pub type PaneFn = fn(&State, &mut Cx<State>, &str, PanePart, &mut Vec<El>) -> El;
/// Draws a view filling its parent (the identity gate).
pub type GateFn = fn(&State, &mut Cx<State>, &mut Vec<El>) -> El;
/// Pushes app-level overlays (the Settings modal) into the layer.
pub type OverlaysFn = fn(&State, &mut Cx<State>, &mut Vec<El>);

/// Which part of the pane [`Pages::pane`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanePart {
    /// One desktop column's body (the shell draws its card, width, strip and resizer).
    Column(PaneTab),
    /// The phone's full-screen sheet, with its own Chat / Properties switch.
    Sheet,
}

/// What the app plugs into the shell (replaces GPUI's `PageFactory`, `set_pane`,
/// `Workspace::on_open_settings` and `Workspace::identity_gate`).
#[derive(Clone, Copy)]
pub struct Pages {
    /// The main area of a route: the desktop's middle column (the shell adds the
    /// header, the scroll and the 920 px column), the full-window `Route::Chat` and
    /// `Route::Site`, the phone's object screen.
    pub page: PageFn,
    /// The open object's pane, while `s.pane.available` and the route is `Route::Object`.
    pub pane: PaneFn,
    /// Shown instead of the workspace while the daemon has no identity.
    pub identity_gate: GateFn,
    /// The rail's account button, ⌘, and [`open_settings`].
    pub open_settings: fn(&mut State),
    /// App-level overlays, drawn over everything the shell draws.
    pub overlays: OverlaysFn,
    /// The visible route changed (or a route opened for the first time), after the
    /// shell reset the pane: an object page publishes its pane here.
    pub shown: fn(&mut State, &Route),
    /// No tab of the active space shows the route any more: drop its page state.
    pub closed: fn(&mut State, &Route),
}

impl Default for Pages {
    fn default() -> Self {
        Pages {
            page: |_, _, _, _| rune::el(),
            pane: |_, _, _, _, _| rune::el(),
            identity_gate: |_, _, _| rune::el(),
            open_settings: |_| {},
            overlays: |_, _, _| {},
            shown: |_, _| {},
            closed: |_, _| {},
        }
    }
}

/// Install the shell: its state (layout read from the stored UI prefs), the app's
/// `pages`, and its listeners (boot space pick, Personal space seeding, legacy
/// space-order move, pin-fold migration, the pinned widgets' loads, the top-pin
/// redirect). Call once, after [`State::new`].
pub fn register(s: &mut State, pages: Pages) {
    workspace::register(s, pages);
    pinned::register(s);
    mobile::register(s);
}

/// Whether pin `id`'s widget shows folded (this device's unsettled fold, else
/// the object's `pin_collapsed`): for tests.
#[doc(hidden)]
pub fn pin_collapsed(s: &State, id: &str) -> bool {
    pin_collapse::collapsed(s, id)
}

/// Fold or unfold pin `id` as its arrow does: for tests.
#[doc(hidden)]
pub fn toggle_pin_collapsed(s: &mut State, id: &str) {
    pin_collapse::toggle(s, id);
}

/// The whole window: the desktop titlebar strip, the workspace (boot states,
/// identity gate, the desk or the phone shell by width) and every overlay.
pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    workspace::view(s, cx)
}

/// The window chrome GPUI's TitleBar asked for: a transparent full-size
/// titlebar, the traffic lights 9 pt in from the top-left, and drag regions
/// (the titlebar strip) so the content under it stays clickable.
pub const CHROME: rune::Chrome = rune::Chrome {
    full_size_content: true,
    traffic_lights: Some((9.0, 9.0)),
    drag_regions: true,
};

/// The window's keys (`AppSpec::on_key`): ⌘K search, ⌘N the New menu, ⌘, Settings
/// (Ctrl elsewhere).
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let chord = |spec: &str| Chord::parse(spec).is_ok_and(|c| c.matches_tx(ev, tx));
    if chord("mod+k") {
        search(s);
        return true;
    }
    if chord("mod+n") {
        new_object(s);
        return true;
    }
    if chord("mod+,") {
        open_settings(s);
        return true;
    }
    false
}

/// The theme for the stored preference (`UiState` key `"theme"`): what every view
/// of the window draws with.
pub fn theme(s: &State, cx: &mut Cx<State>) -> &'static Theme {
    let pref =
        s.ui.get::<String>("theme")
            .map(|name| roostr_ui::ThemePref::from_name(&name))
            .unwrap_or_default();
    roostr_ui::theme(pref, cx)
}

/// Open the search modal (⌘K).
pub fn search(s: &mut State) {
    search::open(s);
}

/// Open the New menu (⌘N).
pub fn new_object(s: &mut State) {
    create_menu::open(s);
}

/// Open Settings through [`Pages::open_settings`].
pub fn open_settings(s: &mut State) {
    if let Some(open) = s.get::<shell::Shell>().map(|sh| sh.pages.open_settings) {
        open(s);
    }
}

/// Navigate to `route` once the shell has picked the space (a path given at
/// launch: `/app/object/<id>`, `/guide`, …).
pub fn open_when_ready(s: &mut State, route: Route) {
    if !s.nav.space().is_empty() {
        roostr_core::nav::navigate(route, s);
    } else {
        s.slot::<shell::Shell>().pending_route = Some(route);
    }
}

/// The window's logical width as the shell's root was last laid out (0 before
/// the first layout).
pub fn window_width(s: &State) -> f32 {
    s.get::<shell::Shell>().map_or(0.0, |sh| sh.width)
}

/// The window's logical height as the shell's root was last laid out (0 before
/// the first layout).
pub fn window_height(s: &State) -> f32 {
    s.get::<shell::Shell>().map_or(0.0, |sh| sh.height)
}

/// The window is phone-narrow (≤ 720 pt): the mobile shell shows.
pub fn is_mobile(s: &State) -> bool {
    let width = window_width(s);
    width > 0.0 && roostr_ui::layout::is_mobile(width)
}
