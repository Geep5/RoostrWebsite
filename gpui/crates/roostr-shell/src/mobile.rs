//! The mobile shell (`+layout.svelte` `{#if isMobile}` markup 911-1238, CSS
//! 2846-3461): at [`crate::is_mobile`] widths the app is the Anytype iOS
//! flow - Spaces list → space home → object - instead of rail | sidebar |
//! main | pane. Every `env(safe-area-inset-*)` term is 0: each host lays the
//! window out inside the safe area and above the keyboard, as the iOS app's
//! web view sat.
//!
//! Touch: Rune turns a finger into presses, drags of elements that drag, and
//! pans of the scroll view under it. The space cards drag (held still, a card
//! lifts for a reorder; moved first, the drag scrolls the list), the left
//! edge drags on touch screens (the back swipe, [`crate::back_swipe`]), and a
//! scroll view pulled down past its top syncs now.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{Local, TimeZone as _};
use roostr_core::State;
use roostr_core::nav::{Route, navigate};
use roostr_core::objects::{is_image_icon, object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::sync_status::{count, sync_progress, sync_tip};
use roostr_core::types::{ObjectSummary, RelationDef, Space, SyncPhase};
use roostr_ui::ty::Weights as _;
use roostr_ui::{CountBadge, Glyph, ProgressCircle, Theme, checkbox, glyph, motion, shadow, ty};
use rune::{
    Color, Cursor, Cx, Drag, Effect, El, LoopSpec, Phase as DragPhase, ScrollSpec, Shadow,
    SpringPair, Transition, Tx, col, el, row, spacer, text,
};
use web_time::Instant;

use crate::PanePart;
use crate::back_swipe::{self, BackSwipe, Phase, Screen, Track};
use crate::shell::{self, current_space, emoji_or_image, fill, fill_glyph, icon_text, or_untitled};
use crate::{list_motion, pin_collapse, pinned};

/// The web app's `__BUILD_STAMP__` (see `build.rs`).
const BUILD_STAMP: &str = env!("ROOSTR_BUILD_STAMP");

/// Pulling a page scroller down this far at its top and letting go syncs now
/// (the iOS web view's pull-to-refresh, which called `Backend.resume`).
const PULL_TO_SYNC: f32 = 64.;
/// How far the content shows pulled down when the finger has pulled
/// [`PULL_TO_SYNC`] past the top: a native pan's rubber band syncs here.
const PULL_SHOWN: f32 = rubber(PULL_TO_SYNC);
/// Holding a space card still this long lifts it (the platform's long press).
const LONG_PRESS: Duration = Duration::from_millis(500);
/// How far a held card may move and still lift: past it the drag scrolls.
const SLOP: f32 = 8.;
/// A back button's first frame (the parent under the screen) before it slides.
const ARM: Duration = Duration::from_millis(16);
/// A released finger's last speed carries a scroll this many seconds on.
const FLING: f32 = 0.325;

/// A scroll view's rubber band: `over` points of finger travel past an edge
/// show as this much (Rune's own touch pan curve).
const fn rubber(over: f32) -> f32 {
    over * 0.55 / (1. + over * 0.55 / 300.)
}

/// The phone shell's state (a slot of the app state).
#[derive(Default)]
pub(crate) struct Mobile {
    /// A space card was tapped open: its home shows instead of the Spaces list.
    space_open: bool,
    /// `mCollapsed`: folded sections of the space home (pinned cards fold on the object).
    collapsed: HashMap<String, bool>,
    /// The space card held up for a reorder (`spaceLiftId`).
    lift: Option<String>,
    /// Where the lifted card drops: (card, after it).
    over: Option<(String, bool)>,
    /// Window rects of the space cards, as last laid out (what a lifted card is over).
    cards: HashMap<String, [f32; 4]>,
    /// A press on a space card in flight.
    press: Option<CardPress>,
    /// The press that just ended was a lift or a scroll: its release opens nothing.
    swallow_press: bool,
    /// The scroll views pull-to-sync watches, by key.
    scrollers: HashMap<&'static str, Scroller>,
    /// A drag from the left edge in flight.
    edge: Option<EdgeDrag>,
    /// The push/pop slide and back swipe.
    back: BackSwipe,
}

/// A scroll view's top as laid out, for its scroll offset and pull.
#[derive(Default)]
struct Scroller {
    /// The viewport's top (window points).
    view_y: Option<f32>,
    /// The content's top (window points), after scrolling.
    top_y: Option<f32>,
    /// A native pull past the top already synced: once per pull.
    synced: bool,
    /// A dragged card or edge pulls the content down by hand this far (finger travel).
    pull: f32,
}

impl Scroller {
    /// The scroll offset (points down from the top; negative while pulled).
    fn offset(&self) -> f32 {
        match (self.view_y, self.top_y) {
            (Some(view), Some(top)) => view - top,
            _ => 0.,
        }
    }
}

/// What a drag that is not a lift or a swipe does: scroll its list by hand.
#[derive(Clone, Copy)]
struct HandScroll {
    list: &'static str,
    /// The list's offset when the press went down.
    base: f32,
    /// The latest finger sample (time, travel) and the finger's speed (pt/s, downward).
    last: (Instant, f32),
    velocity: f32,
}

impl HandScroll {
    fn new(s: &State, list: &'static str, now: Instant) -> HandScroll {
        let base = scroller(s, list).map_or(0., Scroller::offset).max(0.);
        HandScroll {
            list,
            base,
            last: (now, 0.),
            velocity: 0.,
        }
    }

    /// The finger is `dy` below where it went down at `now`: scroll the list
    /// (pulling it down past its top).
    fn step(&mut self, s: &mut State, dy: f32, now: Instant, tx: &mut Tx) {
        let dt = now.saturating_duration_since(self.last.0).as_secs_f32();
        if dt > 0. {
            self.velocity = (dy - self.last.1) / dt;
        }
        self.last = (now, dy);
        let to = self.base - dy;
        let pull = (-to).max(0.);
        s.slot::<Mobile>()
            .scrollers
            .entry(self.list)
            .or_default()
            .pull = pull;
        tx.scroll_to_instant(self.list, 0., to.max(0.));
    }

    /// The finger lifted at `now` (`cancelled`: the system took it): fling on,
    /// and sync when it was pulled far enough.
    fn end(self, s: &mut State, cancelled: bool, now: Instant, tx: &mut Tx) {
        let pull = std::mem::take(
            &mut s
                .slot::<Mobile>()
                .scrollers
                .entry(self.list)
                .or_default()
                .pull,
        );
        tx.animated();
        if cancelled {
            return;
        }
        if pull > PULL_TO_SYNC {
            sync_now(s);
            return;
        }
        let still = now.saturating_duration_since(self.last.0) > Duration::from_millis(60);
        if pull == 0. && !still && self.velocity != 0. {
            let at = (self.base - self.last.1).max(0.);
            tx.scroll_to(self.list, 0., (at - self.velocity * FLING).max(0.));
        }
    }
}

/// A press on a space card.
struct CardPress {
    id: String,
    mode: PressMode,
    /// Lifts the card when the hold lasts (dropping it cancels).
    _hold: roostr_rt::Task<()>,
}

enum PressMode {
    /// Within the slop: lifting it here is a tap; holding it lifts the card.
    Pending,
    /// Held: the card is lifted and follows to its drop.
    Lifted,
    /// Moved before the hold: the drag scrolls the list.
    Scroll(HandScroll),
}

/// A drag from the left edge.
enum EdgeDrag {
    /// Not yet past the slop.
    Pending,
    /// The back swipe: the finger's travel the track has taken.
    Swipe { taken: f32 },
    /// Vertical: scrolls the screen's list by hand.
    Scroll(HandScroll),
    /// Neither (nothing under it scrolls): ignored to its end.
    Ignored,
}

/// The phone shell's listener: the screen shown, for the push slide.
pub(crate) fn register(s: &mut State) {
    s.on_poll(observe_screen);
}

/// A newly shown screen slides in over the screen it was opened from;
/// anything else (a pop landing, a jump) cancels the motion.
fn observe_screen(s: &mut State) -> bool {
    if !crate::is_mobile(s) {
        let m = s.slot::<Mobile>();
        let changed = m.back.shown.is_some();
        m.back = BackSwipe::default();
        return changed;
    }
    let screen = screen(s);
    let prev = s.slot::<Mobile>().back.shown.clone();
    let phase = match &prev {
        Some(prev) if *prev == screen => return false,
        Some(prev) => back_swipe::observe(prev, parent(s, &screen).as_ref(), touch_motion()),
        None => Phase::Idle,
    };
    let pushing = matches!(phase, Phase::Pushing(_));
    let back = &mut s.slot::<Mobile>().back;
    back.shown = Some(screen);
    back.phase = phase;
    back.timer = None;
    if pushing {
        settle_after(s, back_swipe::SETTLED, |s| {
            let back = &mut s.slot::<Mobile>().back;
            if matches!(back.phase, Phase::Pushing(_)) {
                back.phase = Phase::Idle;
            }
        });
    }
    true
}

/// Run `then` after `delay`, replacing the stack's pending step.
fn settle_after(s: &mut State, delay: Duration, then: fn(&mut State)) {
    // Timed from now, not from when the task first runs.
    let timer = s.rt.timer(delay);
    let task = s.rt.spawn(async move |cx| {
        timer.await;
        cx.update(then);
    });
    s.slot::<Mobile>().back.timer = Some(task);
}

/// Phones show no tab strip: entering a space opens its home (`path &&
/// !isMobile`) - the graph stays, it is per space.
pub(crate) fn space_changed(s: &mut State) {
    if !crate::is_mobile(s) {
        return;
    }
    // Its home is the pinned cards: nothing opens by itself here (the
    // desktop, with room for a page, opens the top pin instead).
    let target = match shell::shell(s).map(|sh| &sh.shown_route) {
        Some(route @ Route::Graph { .. }) => route.clone(),
        _ => Route::Home,
    };
    navigate(target, s);
}

/// Whether the phone screens slide and the left edge swipes back: a touch
/// screen (the browser build, where phones run it; the iOS/Android builds).
fn touch_motion() -> bool {
    cfg!(target_family = "wasm") || roostr_core::layout::is_touch()
}

/// The phone screen the route shows.
fn screen(s: &State) -> Screen {
    match s.nav.route() {
        Route::Object(id) => Screen::Object(id.clone()),
        Route::Chat(id) => Screen::Chat(id.clone()),
        Route::Graph { .. } | Route::Site(_) => Screen::Other,
        Route::Home => match current_space(s) {
            Some(space) if s.get::<Mobile>().is_some_and(|m| m.space_open) => {
                Screen::Home(space.id.clone())
            }
            _ => Screen::Spaces,
        },
    }
}

/// The screen `screen` goes back to: an object to its space's home, a
/// chat object's chat likewise, any other chat to its object, a space
/// home to the Spaces list.
fn parent(s: &State, screen: &Screen) -> Option<Screen> {
    let home = |id: &str| {
        let owning = s
            .store
            .summary(id)
            .and_then(|o| s.store.space(&o.channel_id))
            .map(|sp| sp.id.clone());
        Some(
            owning
                .or_else(|| current_space(s).map(|sp| sp.id.clone()))
                .map_or(Screen::Spaces, Screen::Home),
        )
    };
    match screen {
        Screen::Spaces | Screen::Other => None,
        Screen::Home(_) => Some(Screen::Spaces),
        Screen::Object(id) => home(id),
        Screen::Chat(id) => {
            let chat_object = s.store.summary(id).is_some_and(|o| o.type_key == "chat");
            if chat_object {
                home(id)
            } else {
                Some(Screen::Object(id.clone()))
            }
        }
    }
}

/// The stack phase to draw: the stored one, or what the poll will store for
/// a screen it has not seen yet (a handler changed it this frame).
fn phase(s: &State, screen: &Screen) -> Phase {
    let Some(m) = s.get::<Mobile>() else {
        return Phase::Idle;
    };
    match &m.back.shown {
        Some(prev) if prev != screen => {
            back_swipe::observe(prev, parent(s, screen).as_ref(), touch_motion())
        }
        _ => m.back.phase.clone(),
    }
}

/// A back button: slide the screen off to its parent, or go at once.
fn back(s: &mut State, motion: bool) {
    let screen = screen(s);
    let idle = matches!(s.slot::<Mobile>().back.phase, Phase::Idle);
    if motion && idle && parent(s, &screen).is_some() {
        s.slot::<Mobile>().back.phase = Phase::Arming;
        settle_after(s, ARM, |s| {
            if matches!(s.slot::<Mobile>().back.phase, Phase::Arming) {
                pop_off(s);
            }
        });
    } else {
        pop(s);
    }
}

/// Slide the screen off to the right, then show its parent.
fn pop_off(s: &mut State) {
    s.slot::<Mobile>().back.phase = Phase::Popping;
    settle_after(s, back_swipe::SETTLED, |s| {
        if matches!(s.slot::<Mobile>().back.phase, Phase::Popping) {
            s.slot::<Mobile>().back.phase = Phase::Popped;
            pop(s);
        }
    });
}

/// Show the current screen's parent (`mobileBackToSpace` for an object:
/// its space home, the pinned-cards frame, not history - one more back
/// reaches the Spaces list).
fn pop(s: &mut State) {
    let screen = screen(s);
    match parent(s, &screen) {
        Some(Screen::Spaces) => s.slot::<Mobile>().space_open = false,
        Some(Screen::Home(space)) => {
            s.nav.use_space(&space, &mut s.ui);
            s.slot::<Mobile>().space_open = true;
            navigate(Route::Home, s);
        }
        Some(Screen::Object(id)) => navigate(Route::Object(id), s),
        Some(Screen::Chat(_) | Screen::Other) | None => {}
    }
}

/// Sync now where the host can.
fn sync_now(s: &mut State) {
    if let Some(sync) = s.sync_now.clone() {
        (sync.0)(s);
    }
}

fn scroller<'a>(s: &'a State, list: &str) -> Option<&'a Scroller> {
    s.get::<Mobile>().and_then(|m| m.scrollers.get(list))
}

/// `.m-shell`: the screen for the route at a phone width, sliding over
/// its parent while it is pushed, popped or swiped back.
pub(crate) fn view(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
    let width = crate::window_width(s);
    let screen = screen(s);
    let reduced = motion::reduced(cx);
    let motion_on = touch_motion() && !reduced;
    let phase = phase(s, &screen);
    let offset = phase.offset(width, !reduced);
    let parent_screen = parent(s, &screen);
    let settle = motion::release_spring(back_swipe::SETTLE.as_secs_f32());
    let under = back_swipe::under(&phase, parent_screen.clone());

    let mut shell = col()
        .key("m-shell")
        .w_full()
        .grow(1.)
        .min_h(0.)
        .clip_rounded(0.)
        .fill(t.bg);

    if let Some(under) = &under {
        let uncovered = (offset / width.max(1.)).clamp(0., 1.);
        shell = shell
            .child(
                col()
                    .key(under.key())
                    .pin(Some(0.), Some(0.), None, Some(0.))
                    .w(width)
                    .fill(t.bg)
                    .inert()
                    .offset(-(width - offset) * back_swipe::PARALLAX, 0.)
                    .offset_motion(settle)
                    .transition(Transition::none())
                    .child(screen_body(s, cx, t, under, layer, motion_on)),
            )
            .child(
                el().key("m-dim")
                    .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                    .fill(Color::BLACK)
                    .inert()
                    .opacity(back_swipe::DIM * (1. - uncovered))
                    .opacity_motion(settle)
                    .transition(Transition {
                        insertion: Effect::opacity(0.),
                        removal: Effect::IDENTITY,
                        springs: SpringPair::new(settle, rune::Spring::NONE),
                        ..Transition::identity()
                    }),
            );
    }

    // A pushed screen comes in from the right edge; a removed one is gone at once.
    let entrance = match &phase {
        Phase::Pushing(_) => Effect {
            offset: rune::Vec2::new(width, 0.),
            ..Effect::IDENTITY
        },
        _ => Effect::IDENTITY,
    };
    let mut top = col()
        .key(screen.key())
        .grow(1.)
        .min_h(0.)
        .w_full()
        .fill(t.bg)
        .offset(offset, 0.)
        .offset_motion(settle)
        .transition(Transition {
            insertion: entrance,
            removal: Effect::IDENTITY,
            springs: SpringPair::new(settle, rune::Spring::NONE),
            ..Transition::identity()
        })
        .child(screen_body(s, cx, t, &screen, layer, motion_on));
    if phase.shows_under() {
        top = top.box_shadow(Shadow {
            x: -2.,
            y: 0.,
            blur: 24.,
            spread: 0.,
            color: Color::BLACK.alpha(0.18),
            inset: false,
        });
    }
    shell = shell.child(top);

    // The edge swipe sits over the screen, under the sheet (which takes its own pans).
    let lifted = s.get::<Mobile>().is_some_and(|m| m.lift.is_some());
    if touch_motion() && parent_screen.is_some() && !lifted {
        shell = shell.child(edge_strip(&screen, width, reduced));
    }

    // On a phone the chat & properties pane is a full-screen sheet over
    // the object (`.disc-drawer` @media 720).
    if let Route::Object(id) = s.nav.route()
        && s.pane.sheet
        && s.pane.available
        && let Some(pages) = shell::shell(s).map(|sh| sh.pages)
    {
        shell = shell.child(
            col()
                .key("disc-sheet")
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .fill(t.bg)
                .clip_rounded(0.)
                .child((pages.pane)(s, cx, id, PanePart::Sheet, layer)),
        );
    }
    shell
}

/// The left edge's drag (iOS's interactive pop): a pan from it that heads
/// right drags the screen back; one that heads up or down scrolls the
/// screen as the page under it would. Starts below the header row, whose
/// back button it would otherwise cover.
fn edge_strip(screen: &Screen, width: f32, reduced: bool) -> El {
    let list = match screen {
        Screen::Home(_) => Some("m-cards-col"),
        Screen::Object(_) => Some("m-main"),
        _ => None,
    };
    el().key("m-edge")
        .pin(Some(0.), Some(56.), None, Some(0.))
        .w(back_swipe::EDGE)
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
            edge_drag(s, d, tx, list, width, reduced)
        })
}

fn edge_drag(
    s: &mut State,
    d: &mut Drag,
    tx: &mut Tx,
    list: Option<&'static str>,
    width: f32,
    reduced: bool,
) {
    let now = s.rt.now();
    match d.phase {
        DragPhase::Down => {
            let idle = matches!(s.slot::<Mobile>().back.phase, Phase::Idle);
            s.slot::<Mobile>().edge = Some(if idle {
                EdgeDrag::Pending
            } else {
                EdgeDrag::Ignored
            });
        }
        DragPhase::Move => {
            let Some(mut drag) = s.slot::<Mobile>().edge.take() else {
                return;
            };
            if matches!(drag, EdgeDrag::Pending) && d.dx.hypot(d.dy) > SLOP {
                let start = [d.window_point()[0] - d.dx, d.window_point()[1] - d.dy];
                drag = if back_swipe::claims(start, [d.dx, d.dy]) {
                    s.slot::<Mobile>().back.phase = Phase::Tracking(Track::default());
                    EdgeDrag::Swipe { taken: 0. }
                } else if let Some(list) = list {
                    EdgeDrag::Scroll(HandScroll::new(s, list, now))
                } else {
                    EdgeDrag::Ignored
                };
            }
            match &mut drag {
                EdgeDrag::Swipe { taken } => {
                    if let Phase::Tracking(track) = &mut s.slot::<Mobile>().back.phase {
                        track.push(now, d.dx - *taken);
                        *taken = d.dx;
                    }
                }
                EdgeDrag::Scroll(hand) => hand.step(s, d.dy, now, tx),
                EdgeDrag::Pending | EdgeDrag::Ignored => {}
            }
            s.slot::<Mobile>().edge = Some(drag);
        }
        DragPhase::Up => match s.slot::<Mobile>().edge.take() {
            Some(EdgeDrag::Swipe { taken }) => {
                let back = &mut s.slot::<Mobile>().back;
                let Phase::Tracking(track) = &mut back.phase else {
                    return;
                };
                track.push(now, d.dx - taken);
                let goes =
                    !d.cancelled() && back_swipe::commits(track.offset, width, track.velocity(now));
                tx.animated();
                if !goes {
                    back.phase = Phase::Returning;
                    settle_after(s, back_swipe::SETTLED, |s| {
                        let back = &mut s.slot::<Mobile>().back;
                        if back.phase == Phase::Returning {
                            back.phase = Phase::Idle;
                        }
                    });
                } else if reduced {
                    // The screen never followed: go back at once.
                    back.phase = Phase::Popped;
                    pop(s);
                } else {
                    pop_off(s);
                }
            }
            Some(EdgeDrag::Scroll(hand)) => hand.end(s, d.cancelled(), now, tx),
            _ => {}
        },
    }
}

/// A phone screen's content: its header and page.
fn screen_body(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    screen: &Screen,
    layer: &mut Vec<El>,
    motion: bool,
) -> El {
    match screen {
        Screen::Spaces => spaces(s, t),
        Screen::Home(id) => match s.store.space(id).or_else(|| current_space(s)) {
            Some(space) => space_home(s, cx, t, space, layer, motion),
            None => spaces(s, t),
        },
        Screen::Object(id) => object(s, cx, t, id, layer, motion),
        // The full-screen chat brings its own header and keyboard fit.
        Screen::Chat(id) => {
            col()
                .grow(1.)
                .min_h(0.)
                .child(page(s, cx, &Route::Chat(id.clone()), layer))
        }
        Screen::Other => graph(s, cx, t, layer),
    }
}

/// The app's page for `route`.
fn page(s: &State, cx: &mut Cx<State>, route: &Route, layer: &mut Vec<El>) -> El {
    match shell::shell(s).map(|sh| sh.pages) {
        Some(pages) => (pages.page)(s, cx, route, layer),
        None => el(),
    }
}

/// A phone control on the panel: panel fill, 1px border, fg text, a
/// `--hover` fill while pressed. Callers size and round it.
fn panel_button(t: &Theme, key: &str) -> El {
    row()
        .key(key)
        .role("button")
        .center()
        .fill(t.panel)
        .border(1., t.border)
        .border_layout()
        .hover(t.panel)
        .press(t.hover)
        .cursor(Cursor::Pointer)
}

/// `.m-btn`: a 40px round panel button with its label and tooltip.
fn m_btn(t: &Theme, key: &str, label: &str, content: El) -> El {
    let button = panel_button(t, key)
        .size(40., 40.)
        .radius(20.)
        .label(label)
        .child(content);
    roostr_ui::tooltip(button, label)
}

/// The mobile headers' back chevron (20px).
fn back_glyph(t: &Theme) -> El {
    glyph(Glyph::MobileBack, 20.).color(t.fg)
}

/// `.m-top`: the 56px header row (back on the left, actions on the right).
fn m_top(t: &Theme) -> El {
    row()
        .items_center()
        .justify_between()
        .gap(8.)
        .pt(10.)
        .px(12.)
        .pb(10.)
        .min_h(56.)
        .fill(t.bg)
}

/// `.m-obj`: what the header shows, centred on the row (46% wide at most) and
/// transparent to taps.
fn m_obj(s: &State, t: &Theme, icon: Option<String>, name: String) -> El {
    let icon = match icon {
        None => glyph(Glyph::Graph, 15.).color(t.fg),
        Some(icon) if is_image_icon(&icon) => emoji_or_image(s, &icon, 18.),
        Some(icon) => text(icon).font(ty::ui(15.)).color(t.fg),
    };
    row()
        .pin(Some(0.), Some(0.), Some(0.), Some(0.))
        .center()
        .child(
            row()
                .max_w(crate::window_width(s) * 0.46)
                .items_center()
                .gap(8.)
                .child(icon)
                .child(
                    text(name)
                        .font(ty::ui(16.).semibold())
                        .color(t.fg)
                        .truncate()
                        .shrink(),
                ),
        )
}

/// `.m-chev`: the ⌄ of a collapsible card or section, turned over while open
/// (`transition: transform 0.12s`).
fn m_chev(t: &Theme, key: String, open: bool) -> El {
    fill(key, Color::TRANSPARENT)
        .role("button")
        .label("Fold")
        .px(14.)
        .py(12.)
        .child(
            glyph(Glyph::ChevronDown, 13.)
                .color(t.muted)
                .rotate(if open { std::f32::consts::PI } else { 0. })
                .rotate_motion(motion::FADE.anim()),
        )
}

/// `.m-row`: a row of a space-home section as a kit list row (its lead
/// icon or checkbox, the name, a trailing `mark`), `--hover` while pressed.
fn m_row(t: &Theme, key: String, lead: El, name: String, mark: Option<El>) -> El {
    row()
        .key(key)
        .role("button")
        .label(name.clone())
        .w_full()
        .items_center()
        .gap(8.)
        .py(9.)
        .px(6.)
        .radius(roostr_ui::radius::MD)
        .hover(t.kit.list_hover)
        .press(t.hover)
        .cursor(Cursor::Pointer)
        .child(lead)
        .child(text(name).font(ty::ui(14.)).color(t.fg).truncate().shrink())
        .children(mark)
}

/// A space's tile glyph: its icon, else its initial, else "?".
fn space_glyph(space: &Space) -> String {
    if !space.icon.is_empty() {
        return space.icon.clone();
    }
    space
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

/// A space's tile: its glyph centred on a `size` square, `text_size` text.
fn space_tile(s: &State, t: &Theme, space: &Space, size: f32, radius: f32, text_size: f32) -> El {
    row()
        .size(size, size)
        .radius(radius)
        .fill(t.hover)
        .center()
        .clip_rounded(radius)
        .child(icon_text(s, &space_glyph(space), size, ty::ui(text_size)).color(t.fg))
}

/// `shortDate`: the time today, the weekday within a week, else "Oct 5".
fn short_date(ms: i64) -> String {
    let Some(d) = Local.timestamp_millis_opt(ms).earliest() else {
        return String::new();
    };
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        d.format("%-I:%M %p").to_string()
    } else if ((now - d).num_milliseconds() as f64) / 86_400_000. < 7. {
        d.format("%a").to_string()
    } else {
        d.format("%b %-d").to_string()
    }
}

/// A scroll view pull-to-sync watches: `list` keys it (and its scroll
/// requests); a native pan's rubber band past [`PULL_SHOWN`] syncs now, and
/// a hand scroll ([`HandScroll`]) shows its pull as the same band.
fn scroll_view(s: &State, list: &'static str, content: Vec<El>) -> El {
    let pull = scroller(s, list).map_or(0., |sc| sc.pull);
    col()
        .key(list)
        .grow(1.)
        .min_h(0.)
        .w_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .offset(0., rubber(pull))
        .on_layout_tx(move |s: &mut State, r, tx: &mut Tx| {
            s.slot::<Mobile>().scrollers.entry(list).or_default().view_y = Some(r[1]);
            tx.live_only();
        })
        .child(el().key(format!("{list}-top")).w_full().on_layout_tx(
            move |s: &mut State, r, tx: &mut Tx| {
                if !watch_pull(s, list, r[1]) {
                    tx.live_only();
                }
            },
        ))
        .children(content)
}

/// The content of `list` sits at `top_y`: pulled down past its top by more
/// than a sync's pull, sync now (once per pull). True when it synced.
fn watch_pull(s: &mut State, list: &'static str, top_y: f32) -> bool {
    let sc = s.slot::<Mobile>().scrollers.entry(list).or_default();
    sc.top_y = Some(top_y);
    let pulled = -sc.offset();
    if pulled <= 0.5 {
        sc.synced = false;
    } else if pulled > PULL_SHOWN && !sc.synced && sc.pull == 0. {
        sc.synced = true;
        sync_now(s);
        return true;
    }
    false
}

/// `.m-main`: the route's page, scrolling under the header.
fn m_main(s: &State, cx: &mut Cx<State>, route: &Route, layer: &mut Vec<El>) -> El {
    let page = page(s, cx, route, layer);
    scroll_view(
        s,
        "m-main",
        vec![col().w_full().pt(24.).pb(80.).child(page)],
    )
}

/// `/app/graph`: back (history) and the Graph title.
fn graph(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
    let route = s.nav.route().clone();
    col()
        .grow(1.)
        .min_h(0.)
        .child(
            m_top(t)
                .child(m_obj(s, t, None, "Graph".into()))
                .child(
                    m_btn(t, "m-back", "Back", back_glyph(t))
                        .on_press(|s: &mut State| s.nav.back(&mut s.ui)),
                )
                .child(spacer()),
        )
        .child(m_main(s, cx, &route, layer))
}

/// An object: back to its space home, its icon and name, then the sync
/// dot, ⋯ and the chat & properties button as fit what it is.
fn object(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    id: &str,
    layer: &mut Vec<El>,
    motion: bool,
) -> El {
    let route = Route::Object(id.to_string());
    let (path_icon, path_name, graph) = crate::header::header_path(s, &route);
    let relation = s.store.relations.iter().any(|r| r.id == id);
    let summary = s.store.summary(id).is_some();
    let space = !summary && s.store.space(id).is_some();
    let pane_btn = (s.pane.available && (summary || space)).then(|| {
        let button = m_btn(
            t,
            "m-pane",
            "Chat & properties",
            glyph(Glyph::Pane, 18.).color(t.fg),
        )
        .on_press(|s: &mut State| s.pane.set_sheet(true));
        CountBadge::new(button)
            .count(s.pane.count)
            .max(99)
            .color(t.accent)
            .build(t)
    });
    let sync = (relation || summary).then(|| m_sync(s, t));
    let actions = (relation || summary || space).then(|| {
        row()
            .items_center()
            .gap(8.)
            .children(sync)
            .child(
                row().key("more").child(
                    m_btn(t, "m-more", "More", text("⋯").font(ty::ui(19.)).color(t.fg))
                        .on_press(crate::header::toggle_more),
                ),
            )
            .children(pane_btn)
    });
    col()
        .grow(1.)
        .min_h(0.)
        .child(
            m_top(t)
                .child(m_obj(s, t, (!graph).then_some(path_icon), path_name))
                .child(
                    m_btn(t, "m-back", "Space home", back_glyph(t))
                        .on_press(move |s: &mut State| back(s, motion)),
                )
                .children(actions),
        )
        .child(m_main(s, cx, &route, layer))
}

/// `.m-sync`: the sync dot as a button - green when synced, a pulsing
/// orange while busy, a progress ring (filled to the pass's percentage,
/// or a spinning arc) and its label while a history pass runs. A tap
/// syncs now where the host can. The dot cross-fades between states.
fn m_sync(s: &State, t: &Theme) -> El {
    let status = &s.store.status;
    let pending = status.pending.unwrap_or(0);
    let ok = status.phase == SyncPhase::Live && pending == 0;
    let busy = status.phase == SyncPhase::Backfill || pending > 0;
    let progress = sync_progress(status);
    let pct = progress.as_ref().and_then(|p| p.pct);
    let dot_color = if ok {
        t.green
    } else if busy {
        t.orange
    } else {
        t.muted
    };
    let mut dot = el()
        .key("m-sync-dot")
        .size(12., 12.)
        .radius(6.)
        .fill(dot_color)
        .motion(motion::Tween::ease(200).spring());
    if busy && pct.is_none() {
        dot = dot.looping(LoopSpec::opacity(1.2, 0., 0.35, 1.));
    }
    let mut button = panel_button(t, "m-sync")
        .size(40., 40.)
        .radius(20.)
        .border(0., Color::TRANSPARENT)
        .label("Sync");
    // The 20px ring around the dot: filled to the pass's percentage, or an
    // indeterminate arc.
    if progress.is_some() {
        button = button.child(
            row()
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .center()
                .child(
                    ProgressCircle::new("m-sync-ring")
                        .color(t.orange)
                        .loading(pct.is_none())
                        .value(pct.map_or(0., |pct| pct as f32))
                        .px(20.)
                        .build(t),
                ),
        );
    }
    button = button.child(dot);
    if let Some(p) = &progress {
        let label = match pct {
            Some(pct) => format!("{pct}%"),
            None => count(p.received),
        };
        button = button.child(
            row()
                .pin(Some(0.), None, Some(0.), Some(1.))
                .justify_center()
                .child(text(label).font(ty::ui(8.).lh(8.)).color(t.muted)),
        );
    }
    button = if s.sync_now.is_some() {
        button.on_press(sync_now)
    } else {
        button.cursor(Cursor::Default)
    };
    roostr_ui::tooltip(button, sync_tip(status))
}

/// Screen 1: the Spaces list (avatar → Settings, the cards in this
/// person's order, held to reorder, New space, the build stamp).
fn spaces(s: &State, t: &Theme) -> El {
    let spaces: Vec<&Space> = s.store.ordered_spaces();
    let m = s.get::<Mobile>();
    let lift = m.and_then(|m| m.lift.clone());
    let over = m.and_then(|m| m.over.clone()).filter(|_| lift.is_some());
    let status = &s.store.status;

    let mut cards: Vec<El> = Vec::new();
    for space in spaces {
        // `latestInChannel`: the space's most recently touched object.
        let latest = s
            .store
            .summaries
            .iter()
            .filter(|o| o.channel_id == space.id)
            .fold(None::<&ObjectSummary>, |best, o| match best {
                Some(b) if o.updated_at <= b.updated_at => Some(b),
                _ => Some(o),
            });
        cards.push(space_card(
            s,
            t,
            space,
            latest,
            lift.as_deref(),
            over.as_ref(),
        ));
    }
    cards.push(
        panel_button(t, "m-new-space")
            .w_full()
            .padding(12.)
            .radius(10.)
            .label("New space")
            .child(text("＋ New space").font(ty::ui(16.)).color(t.muted))
            .on_press(crate::rail::new_space),
    );
    let cards = scroll_view(
        s,
        "m-cards",
        vec![col().w_full().gap(10.).pb(12.).children(cards)],
    );

    let importing = (!status.bootstrapped && status.phase != SyncPhase::Idle).then(|| {
        let detail = status
            .detail
            .clone()
            .unwrap_or_else(|| format!("{} changes imported", count(status.imported)));
        let message = format!("Loading your vault… {detail}. Spaces appear as they arrive.");
        // The kit's info alert, bare: no padding, border or fill, muted 12px.
        row()
            .key("m-importing")
            .role("alert")
            .mx(16.)
            .mb(8.)
            .gap(6.)
            .items_start()
            .child(
                glyph(Glyph::Info, roostr_ui::rems(0.875))
                    .color(t.muted)
                    .mt(2.),
            )
            .child(
                text(message)
                    .font(ty::ui(12.).lh(12. * 1.35))
                    .color(t.muted)
                    .wrap()
                    .shrink()
                    .grow(1.),
            )
    });

    let avatar = fill("m-avatar", Color::TRANSPARENT)
        .role("button")
        .label("Settings")
        .size(36., 36.)
        .radius(18.)
        .child(
            crate::rail::account_avatar(s, t, 36., Glyph::Settings)
                .fill(t.hover)
                .border(1., t.border),
        )
        .on_press(crate::open_settings);
    col()
        .key("m-spaces")
        .grow(1.)
        .min_h(0.)
        .pt(12.)
        .px(14.)
        .child(
            col()
                .mb(14.)
                .child(
                    row()
                        .justify_end()
                        .pb(10.)
                        .child(roostr_ui::tooltip(avatar, "Settings")),
                )
                .child(
                    roostr_ui::spaced_text("Spaces", -0.01)
                        .size(32.)
                        .weight(ty::BOLD)
                        .color(t.fg)
                        .build(t),
                ),
        )
        .children(importing)
        .child(cards)
        .child(
            row().mb(4.).justify_center().opacity(0.6).child(
                text(format!("Build {BUILD_STAMP}"))
                    .font(ty::ui(10.))
                    .color(t.muted),
            ),
        )
}

/// A space card: tile, name, the latest object and when it changed. Tapped
/// it opens the space's home; held it lifts for a reorder.
fn space_card(
    s: &State,
    t: &Theme,
    space: &Space,
    latest: Option<&ObjectSummary>,
    lift: Option<&str>,
    over: Option<&(String, bool)>,
) -> El {
    let id = space.id.clone();
    let lifted = lift == Some(id.as_str());
    let mut names = col().grow(1.).shrink().min_w(0.).gap(2.).child(
        text(space.name.clone())
            .font(ty::ui(16.).semibold())
            .color(t.fg)
            .truncate(),
    );
    if let Some(latest) = latest {
        let icon = if latest.icon.is_empty() {
            String::new()
        } else {
            format!("{} ", latest.icon)
        };
        names = names.child(
            text(format!("{icon}{}", or_untitled(&latest.name)))
                .font(ty::ui(13.))
                .color(t.muted)
                .truncate(),
        );
    }
    let mut card = row()
        .key(format!("m-card-{id}"))
        .role("button")
        .label(space.name.clone())
        .items_center()
        .gap(12.)
        .fill(t.panel)
        .border(1., if lifted { t.accent } else { t.border })
        .border_layout()
        .radius(10.)
        .padding(14.)
        .min_h(64.)
        .cursor(Cursor::Pointer)
        .child(space_tile(s, t, space, 44., 10., 22.))
        .child(names);
    if lifted {
        card = card.box_shadow(shadow::drag());
    }
    if let Some(latest) = latest {
        card = card.child(
            text(short_date(latest.updated_at))
                .font(ty::ui(12.))
                .color(t.muted),
        );
    }
    if let Some((_, after)) = over.filter(|(o, _)| *o == id) {
        let (top, bottom) = if *after {
            (None, Some(-5.))
        } else {
            (Some(-5.), None)
        };
        card = card.child(
            el().pin(Some(8.), top, Some(8.), bottom)
                .h(2.)
                .radius(2.)
                .fill(t.accent),
        );
    }
    let layout_id = id.clone();
    let drag_id = id.clone();
    card.on_layout_tx(move |s: &mut State, r, tx: &mut Tx| {
        s.slot::<Mobile>().cards.insert(layout_id.clone(), r);
        tx.live_only();
    })
    .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| card_drag(s, &drag_id, d, tx))
    .on_press(move |s: &mut State| {
        let m = s.slot::<Mobile>();
        // A completed lift (or a scroll) is never navigation.
        if std::mem::take(&mut m.swallow_press) || m.lift.is_some() {
            return;
        }
        s.nav.use_space(&id, &mut s.ui);
        s.slot::<Mobile>().space_open = true;
    })
}

/// A press on a space card: held still it lifts the card (moving the finger
/// marks where it drops, releasing commits the move); moved first it
/// scrolls the list.
fn card_drag(s: &mut State, id: &str, d: &mut Drag, tx: &mut Tx) {
    let now = s.rt.now();
    match d.phase {
        DragPhase::Down => {
            let held = id.to_string();
            let timer = s.rt.timer(LONG_PRESS);
            let hold = s.rt.spawn(async move |cx| {
                timer.await;
                cx.update(move |s| {
                    let m = s.slot::<Mobile>();
                    let Some(press) = m.press.as_mut() else {
                        return;
                    };
                    if press.id == held && matches!(press.mode, PressMode::Pending) {
                        press.mode = PressMode::Lifted;
                        m.lift = Some(held);
                        m.over = None;
                    }
                });
            });
            let m = s.slot::<Mobile>();
            m.swallow_press = false;
            m.press = Some(CardPress {
                id: id.to_string(),
                mode: PressMode::Pending,
                _hold: hold,
            });
        }
        DragPhase::Move => {
            let Some(mut press) = s.slot::<Mobile>().press.take() else {
                return;
            };
            if matches!(press.mode, PressMode::Pending) && d.dx.hypot(d.dy) > SLOP {
                press.mode = PressMode::Scroll(HandScroll::new(s, "m-cards", now));
            }
            match &mut press.mode {
                PressMode::Lifted => {
                    let [x, y] = d.window_point();
                    let m = s.slot::<Mobile>();
                    let target = m
                        .cards
                        .iter()
                        .find(|(_, r)| {
                            x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
                        })
                        .map(|(card, r)| (card.clone(), y > r[1] + r[3] / 2.));
                    if let Some((card, after)) = target
                        && Some(&card) != m.lift.as_ref()
                    {
                        m.over = Some((card, after));
                    }
                }
                PressMode::Scroll(hand) => hand.step(s, d.dy, now, tx),
                PressMode::Pending => {}
            }
            s.slot::<Mobile>().press = Some(press);
        }
        DragPhase::Up => {
            let Some(press) = s.slot::<Mobile>().press.take() else {
                return;
            };
            match press.mode {
                PressMode::Lifted => {
                    let m = s.slot::<Mobile>();
                    m.swallow_press = true;
                    let lift = m.lift.take();
                    let over = m.over.take();
                    if let (Some(lift), Some((target, after)), false) = (lift, over, d.cancelled())
                    {
                        crate::rail::commit_space_move(s, &lift, &target, after);
                    }
                }
                PressMode::Scroll(hand) => {
                    s.slot::<Mobile>().swallow_press = true;
                    hand.end(s, d.cancelled(), now, tx);
                }
                PressMode::Pending => {}
            }
        }
    }
}

/// Fold or unfold the space-home section `key`.
fn flip_section(s: &mut State, key: &str) {
    let m = s.slot::<Mobile>();
    let collapsed = !m.collapsed.get(key).copied().unwrap_or(false);
    m.collapsed.insert(key.to_string(), collapsed);
}

/// `.m-section`: an uppercase label and ⌄ that fold it (with `add`
/// between them while open), over a card of rows.
fn m_section(
    s: &State,
    t: &Theme,
    key: &'static str,
    label: &'static str,
    add: Option<El>,
    rows: Vec<El>,
) -> El {
    let open = !s
        .get::<Mobile>()
        .and_then(|m| m.collapsed.get(key).copied())
        .unwrap_or(false);
    // The body folds with the ⌄; a long section only fades.
    let slide = rows.len() <= 30;
    let body = open.then(|| {
        col()
            .mb(6.)
            .fill(t.panel)
            .border(1., t.border)
            .border_layout()
            .radius(10.)
            .pt(4.)
            .px(10.)
            .pb(8.)
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                let cell = col().w_full().child(row);
                if i > 0 {
                    cell.border_sides([1., 0., 0., 0.], t.border)
                        .border_layout()
                } else {
                    cell
                }
            }))
    });
    col()
        .key(format!("m-section-{key}"))
        .child(
            row()
                .items_center()
                .child(
                    row()
                        .key(format!("m-section-label-{key}"))
                        .role("button")
                        .grow(1.)
                        .items_center()
                        .pt(10.)
                        .px(4.)
                        .pb(6.)
                        .cursor(Cursor::Pointer)
                        .child(
                            roostr_ui::spaced_text(label.to_uppercase(), 0.05)
                                .size(13.)
                                .weight(ty::SEMIBOLD)
                                .color(t.muted)
                                .build(t),
                        )
                        .on_press(move |s: &mut State| flip_section(s, key)),
                )
                .children(add.filter(|_| open))
                .child(
                    m_chev(t, format!("m-section-chev-{key}"), open)
                        .on_press(move |s: &mut State| flip_section(s, key)),
                ),
        )
        .child(list_motion::fold(
            format!("m-section-fold-{key}"),
            true,
            slide,
            body,
        ))
}

/// `.m-section-add`: a 44px ＋ target.
fn m_section_add(t: &Theme, key: &str, label: &str) -> El {
    fill(key, Color::TRANSPARENT)
        .role("button")
        .label(label)
        .size(44., 44.)
        .child(fill_glyph(t, Glyph::Plus, 17.))
}

/// Screen 2: a space's home - its pinned objects as cards, Recently
/// edited, Types (only creatable here on a phone: the bottom ＋ makes
/// objects), Properties, and the bottom bar (Search, ＋ New object).
fn space_home(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    current: &Space,
    layer: &mut Vec<El>,
    motion: bool,
) -> El {
    let active = s.nav.space().to_string();
    let space_id = shell::space_id(s);
    let card_width = crate::window_width(s) - 28.;

    let mut column: Vec<El> = Vec::new();
    // Lite: the vault or this space not ready yet, or the space loading.
    if let Some(notice) = roostr_core::lite_ui::ready_notice(&current.id, s) {
        column.push(
            row()
                .px(14.)
                .child(crate::sidebar::notice_row(t, notice, "m-ready-notice")),
        );
    }

    // Pinned cards.
    let mut pins: Vec<El> = Vec::new();
    for p in current
        .pinned_ids
        .iter()
        .filter_map(|id| s.store.summary(id))
    {
        pins.push(list_motion::pin_card(
            s,
            "m-pins",
            &p.id,
            pinned_card(s, cx, t, p, &active, card_width, layer),
        ));
    }
    column.push(list_motion::pin_list(
        s,
        "m-pins",
        col().w_full().gap(10.).children(pins),
    ));

    // Recently edited: the space's newest six, pinned or not.
    let mut recents: Vec<&ObjectSummary> = s
        .store
        .summaries
        .iter()
        .filter(|o| o.channel_id == current.id)
        .collect();
    recents.sort_by_key(|o| std::cmp::Reverse(o.updated_at));
    let recent_rows: Vec<El> = recents
        .into_iter()
        .take(6)
        .map(|o| {
            let lead = if s.store.layout_of(&o.type_key) == "task" {
                let id = o.id.clone();
                checkbox(format!("m-row-check-{}", o.id), o.done == Some(true))
                    .on_change(move |s: &mut State, done| crate::sidebar::set_done(s, &id, done))
                    .build(t)
                    .mr(6.)
            } else {
                let icon = if o.icon.is_empty() {
                    type_glyph(&s.store, &o.type_key, &active)
                } else {
                    o.icon.clone()
                };
                shell::obj_icon(s, t, &icon)
            };
            let id = o.id.clone();
            m_row(
                t,
                format!("m-recent-{}", o.id),
                lead,
                or_untitled(&o.name),
                None,
            )
            .on_press(move |s: &mut State| navigate(Route::Object(id.clone()), s))
        })
        .collect();
    column.push(m_section(
        s,
        t,
        "__recent",
        "Recently edited",
        None,
        recent_rows,
    ));

    // Types.
    let type_rows: Vec<El> = s
        .store
        .types
        .iter()
        .filter(|ty| ty.space == space_id)
        .map(|ty| {
            let icon = if ty.icon.is_empty() {
                type_glyph(&s.store, &ty.key, &active)
            } else {
                ty.icon.clone()
            };
            let name = if ty.name.is_empty() {
                ty.key.clone()
            } else {
                ty.name.clone()
            };
            let id = ty.id.clone();
            let mark = ty
                .builtin
                .then(|| shell::builtin_mark(t, format!("m-type-lock-{}", ty.id)));
            m_row(
                t,
                format!("m-type-{}", ty.id),
                shell::obj_icon(s, t, &icon),
                name,
                mark,
            )
            .on_press(move |s: &mut State| navigate(Route::Object(id.clone()), s))
        })
        .collect();
    let add_type = m_section_add(t, "m-new-type", "New type").on_press(crate::sidebar::new_type);
    column.push(m_section(
        s,
        t,
        "__types",
        "Types",
        Some(add_type),
        type_rows,
    ));

    // Properties.
    let mut props: Vec<&RelationDef> = space_relations(&s.store.relations, &space_id)
        .into_iter()
        .filter(|r| !r.hidden && !is_reserved_key(&r.key))
        .collect();
    props.sort_by_key(|r| {
        if r.name.is_empty() {
            r.key.to_lowercase()
        } else {
            r.name.to_lowercase()
        }
    });
    let prop_rows: Vec<El> = props
        .into_iter()
        .map(|r| {
            let icon = r
                .icon_emoji
                .clone()
                .filter(|i| !i.is_empty())
                .unwrap_or_else(|| format_glyph(&r.format).to_string());
            let name = if r.name.is_empty() {
                r.key.clone()
            } else {
                r.name.clone()
            };
            let id = r.id.clone();
            let mark = s
                .store
                .builtin_ids
                .contains(&r.id)
                .then(|| shell::builtin_mark(t, format!("m-prop-lock-{}", r.id)));
            m_row(
                t,
                format!("m-prop-{}", r.id),
                shell::obj_icon(s, t, &icon),
                name,
                mark,
            )
            .on_press(move |s: &mut State| navigate(Route::Object(id.clone()), s))
        })
        .collect();
    // The New property suggest anchors under it.
    let add_prop = row().key("new-prop").child(
        m_section_add(t, "m-new-prop", "New property").on_press(crate::sidebar::open_new_property),
    );
    column.push(m_section(
        s,
        t,
        "__props",
        "Properties",
        Some(add_prop),
        prop_rows,
    ));

    let settings_id = current.id.clone();
    col()
        .key("m-space-home")
        .grow(1.)
        .min_h(0.)
        .child(
            m_top(t)
                .child(
                    m_btn(t, "m-spaces", "Spaces", back_glyph(t))
                        .on_press(move |s: &mut State| back(s, motion)),
                )
                .child(spacer())
                .child(
                    m_btn(
                        t,
                        "m-space-settings",
                        "Space settings",
                        glyph(Glyph::Settings, 18.).color(t.fg),
                    )
                    .on_press(move |s: &mut State| navigate(Route::Object(settings_id.clone()), s)),
                ),
        )
        .child(
            col()
                .grow(1.)
                .min_h(0.)
                .px(14.)
                .child(
                    row()
                        .items_center()
                        .gap(12.)
                        .pt(8.)
                        .px(4.)
                        .pb(16.)
                        .child(space_tile(s, t, current, 52., 12., 26.))
                        .child(
                            col()
                                .min_w(0.)
                                .shrink()
                                .child(
                                    text(current.name.clone())
                                        .font(ty::ui(20.).bold())
                                        .color(t.fg),
                                )
                                .child(
                                    text(if current.members.is_empty() {
                                        "Private space"
                                    } else {
                                        "Shared space"
                                    })
                                    .font(ty::ui(13.))
                                    .color(t.muted),
                                ),
                        ),
                )
                .child(scroll_view(
                    s,
                    "m-cards-col",
                    vec![
                        col()
                            .key("m-cards-col-body")
                            .w_full()
                            .gap(10.)
                            .pb(12.)
                            .children(column),
                    ],
                ))
                .child(
                    row()
                        .gap(10.)
                        .pt(10.)
                        .pb(12.)
                        .child(
                            panel_button(t, "m-search")
                                .grow(1.)
                                .justify_start()
                                .radius(999.)
                                .px(16.)
                                .py(12.)
                                .label("Search")
                                .child(text("⌕ Search").font(ty::ui(15.)).color(t.muted))
                                .on_press(crate::search::open),
                        )
                        .child(roostr_ui::tooltip(
                            panel_button(t, "m-compose")
                                .size(46., 46.)
                                .radius(23.)
                                .label("New object")
                                .child(glyph(Glyph::Plus, 22.).color(t.fg))
                                .on_press(crate::create_menu::toggle),
                            "New object",
                        )),
                ),
        )
}

/// A pinned object's card on the space home: its icon and name (opens it),
/// the ⌄ that folds it, and its widget below while open. Only this
/// device's own fold moves (`.m-wcard-body` popped in Svelte); one sync
/// brings in snaps.
fn pinned_card(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    p: &ObjectSummary,
    active: &str,
    card_width: f32,
    layer: &mut Vec<El>,
) -> El {
    let id = p.id.clone();
    let collapsed = pin_collapse::collapsed(s, &id);
    let own = pin_collapse::own(s, &id);
    let icon = object_icon(&s.store, &p.icon, &p.type_key, active);
    let name = or_untitled(&p.name);
    // A widget with no preview (notes, pages) has no body at all.
    let body = (!collapsed && !pinned::is_empty(s, &id)).then(|| {
        // The body column inside the card's border and 10 pt padding (the gallery
        // fits its columns to it).
        let width = card_width - 2. - 20.;
        col()
            .w_full()
            .px(10.)
            .pb(10.)
            .child(pinned::widget(s, cx, t, &id, width, layer))
    });
    let open_id = id.clone();
    let flip_id = id.clone();
    col()
        .key(format!("m-wcard-box-{id}"))
        .w_full()
        .fill(t.panel)
        .border(1., t.border)
        .border_layout()
        .radius(10.)
        .clip_rounded(10.)
        .child(
            row()
                .w_full()
                .items_center()
                .child(
                    fill(format!("m-wcard-{id}"), Color::TRANSPARENT)
                        .role("button")
                        .label(name.clone())
                        .grow(1.)
                        .min_w(0.)
                        .justify_start()
                        .gap(10.)
                        .pt(12.)
                        .pr(6.)
                        .pb(12.)
                        .pl(16.)
                        .child(
                            row().w(22.).center().child(
                                icon_text(s, &icon, 16., ty::ui(16.).lh(16.)).color(t.accent),
                            ),
                        )
                        .child(
                            text(name)
                                .font(ty::ui(15.).semibold())
                                .color(t.fg)
                                .truncate()
                                .shrink()
                                .grow(1.),
                        )
                        .on_press(move |s: &mut State| navigate(Route::Object(open_id.clone()), s)),
                )
                .child(
                    m_chev(t, format!("m-wcard-chev-{id}"), !collapsed)
                        .on_press(move |s: &mut State| pin_collapse::toggle(s, &flip_id)),
                ),
        )
        .child(list_motion::fold(
            format!("m-wcard-fold-{id}"),
            own,
            true,
            body,
        ))
}
