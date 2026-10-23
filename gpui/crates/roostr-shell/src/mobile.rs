//! The mobile shell (`+layout.svelte` `{#if isMobile}` markup 911-1238, CSS
//! 2846-3461): at [`layout::is_mobile`] widths the app is the Anytype iOS
//! flow - Spaces list → space home → object - instead of rail | sidebar |
//! main | pane. Every `env(safe-area-inset-*)` term is 0: each host lays the
//! window out inside the safe area and above the keyboard, as the iOS app's
//! web view sat.

use std::time::Duration;

use chrono::{Local, TimeZone as _};
use gpui_kit::component::Icon;
use gpui_kit::component::IconNamed as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::layout::{self, sync_now};
use roostr_core::motion;
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::objects::{is_image_icon, object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::store::Store;
use roostr_core::sync_status::{count, sync_progress, sync_tip};
use roostr_core::types::{ObjectSummary, RelationDef, Space, SyncPhase};
use roostr_core::{Glyph, PaneState, theme, ui};

use crate::PaneHost;
use crate::Tip as _;
use crate::back_swipe::{self, Phase, Screen, Track};
use crate::list_motion;
use crate::pinned::PinnedWidget;
use crate::rail::commit_space_move;
use crate::sidebar::builtin_mark;
use crate::workspace::Workspace;

/// The web app's `__BUILD_STAMP__` (see `build.rs`).
const BUILD_STAMP: &str = env!("ROOSTR_BUILD_STAMP");

/// Pulling a page scroller down this far at its top and letting go syncs now
/// (the iOS web view's pull-to-refresh, which called `Backend.resume`).
const PULL_TO_SYNC: f32 = 64.;

/// A phone control on the panel: panel fill, 1px border, fg text, a
/// `--hover` fill while pressed. Callers size and round it.
fn panel_button(id: impl Into<ElementId>, cx: &App) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(theme::border())
                .foreground(theme::fg())
                .hover(theme::panel())
                .active(theme::hover())
                .shadow(false),
        )
        .relative()
        .flex_none()
        .p_0()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .text_color(theme::fg())
        .cursor_pointer()
}

/// `.m-btn`: a 40px round panel button.
fn m_btn(id: &'static str, cx: &App) -> Button {
    panel_button(id, cx)
        .debug_selector(move || id.into())
        .size(px(40.))
        .rounded(px(20.))
}

/// The mobile headers' back chevron (20px).
fn back_glyph() -> Icon {
    Icon::new(Glyph::MobileBack).size(px(20.))
}

/// `.m-top`: the 56px header row (back on the left, actions on the right).
fn m_top() -> Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .pt(px(10.))
        .px(px(12.))
        .pb(px(10.))
        .min_h(px(56.))
        .bg(theme::bg())
}

/// `.m-obj`: what the header shows, centred on the row (46% wide at most) and
/// transparent to taps.
fn m_obj(icon: Option<String>, name: String) -> Div {
    let icon = match icon {
        None => svg()
            .path(Glyph::Graph.path())
            .size(px(15.))
            .flex_none()
            .text_color(theme::fg())
            .into_any_element(),
        Some(icon) if is_image_icon(&icon) => ui::emoji_or_image_icon(&icon, px(18.)),
        Some(icon) => div()
            .flex_none()
            .text_size(px(15.))
            .child(icon)
            .into_any_element(),
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .max_w(relative(0.46))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon)
                .child(div().min_w_0().truncate().child(name)),
        )
}

/// `.m-chev`: the ⌄ of a collapsible card or section, turned over while open
/// (`transition: transform 0.12s`): `turn` 0 folded, 1 open.
fn m_chev(id: impl Into<ElementId>, turn: f32, cx: &App) -> Button {
    crate::fill_button(id, transparent_black(), cx)
        .h_auto()
        .px(px(14.))
        .py(px(12.))
        .accessibility_label("Fold")
        .child(
            svg()
                .path(Glyph::ChevronDown.path())
                .size(px(13.))
                .text_color(theme::muted())
                .when(turn > 0., |s| {
                    s.with_transformation(Transformation::rotate(radians(
                        std::f32::consts::PI * turn,
                    )))
                }),
        )
}

/// `.m-row`: a row of a space-home section as a kit list row (its lead
/// icon or checkbox, the name, a trailing `mark`), `--hover` while pressed.
fn m_row(
    id: impl Into<ElementId>,
    lead: AnyElement,
    name: String,
    mark: Option<Stateful<Div>>,
) -> ListItem {
    ListItem::new(id)
        .py(px(9.))
        .px(px(6.))
        .rounded(theme::radius::md())
        .text_size(px(14.))
        .text_color(theme::fg())
        .cursor_pointer()
        .active(|s| s.bg(theme::hover()))
        .accessibility_label(name.clone())
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(lead)
                .child(div().min_w_0().truncate().child(name))
                .children(mark),
        )
}

/// `.obj-icon` at its 20px width.
fn row_icon(icon: &str) -> Div {
    crate::sidebar::obj_icon(icon)
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

impl Workspace {
    /// `.m-shell`: the screen for the route at a phone width, sliding over
    /// its parent while it is pushed, popped or swiped back
    /// ([`crate::back_swipe`]).
    pub(crate) fn render_mobile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let route = Navigator::global(cx).read(cx).route().clone();
        let screen = self.mobile_screen(&route, cx);
        let width = f32::from(window.viewport_size().width);
        self.back_observe(&screen, width, cx);
        let offset = self.back_offset(width, window, cx);
        let under = if offset > 0. {
            self.back_under(&screen, cx)
        } else {
            None
        };
        let body = self.render_screen(&screen, &route, window, cx);
        let under = under.map(|under| {
            let progress = (offset / width).clamp(0., 1.);
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(-(width - offset) * back_swipe::PARALLAX))
                .w(px(width))
                .flex()
                .flex_col()
                .bg(theme::bg())
                .line_height(relative(1.2))
                .child(self.render_screen(&under, &route, window, cx))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(black().opacity(back_swipe::DIM * (1. - progress))),
                )
        });
        let chat = matches!(screen, Screen::Chat(_));
        let top = div()
            .relative()
            .left(px(offset))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .when(!chat, |d| d.line_height(relative(1.2)))
            .when(offset > 0., |d| {
                d.occlude().shadow(vec![BoxShadow {
                    color: black().opacity(0.18),
                    offset: point(px(-2.), px(0.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                    inset: false,
                }])
            })
            .child(body);
        // On a phone the chat & properties pane is a full-screen sheet over
        // the object (`.disc-drawer` @media 720).
        let pane = PaneState::global(cx).read(cx);
        let (sheet_open, pane_available) = (pane.sheet, pane.available);
        let sheet = (matches!(route, Route::Object(_)) && sheet_open && pane_available)
            .then(|| {
                PaneHost::global(cx)
                    .read(cx)
                    .views
                    .as_ref()
                    .map(|v| v.sheet.clone())
            })
            .flatten()
            .map(|view| {
                div()
                    .id("disc-sheet")
                    .debug_selector(|| "disc-sheet".into())
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .bg(theme::bg())
                    .overflow_hidden()
                    .occlude()
                    .child(view)
            });
        div()
            .id("m-shell")
            .debug_selector(|| "m-shell".into())
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme::bg())
            .text_color(theme::fg())
            .children(under)
            .child(top)
            // Under the sheet, which takes its own pans.
            .when(cfg!(target_os = "ios"), |d| {
                d.child(self.back_swipe_listener(cx))
            })
            .children(sheet)
            .into_any_element()
    }

    /// The phone screen `route` shows.
    fn mobile_screen(&self, route: &Route, cx: &App) -> Screen {
        match route {
            Route::Object(id) => Screen::Object(id.clone()),
            Route::Chat(id) => Screen::Chat(id.clone()),
            Route::Graph { .. } | Route::Site(_) => Screen::Other,
            Route::Home => match Self::current_space(cx) {
                Some(space) if self.mobile_space_open => Screen::Home(space.id),
                _ => Screen::Spaces,
            },
        }
    }

    /// The screen `screen` goes back to: an object to its space's home, a
    /// chat object's chat likewise, any other chat to its object, a space
    /// home to the Spaces list.
    fn mobile_parent(screen: &Screen, cx: &App) -> Option<Screen> {
        let home = |id: &str| {
            let owning = {
                let store = Store::global(cx).read(cx);
                store
                    .summary(id)
                    .and_then(|s| store.space(&s.channel_id))
                    .map(|s| s.id.clone())
            };
            Some(
                owning
                    .or_else(|| Self::current_space(cx).map(|s| s.id))
                    .map_or(Screen::Spaces, Screen::Home),
            )
        };
        match screen {
            Screen::Spaces | Screen::Other => None,
            Screen::Home(_) => Some(Screen::Spaces),
            Screen::Object(id) => home(id),
            Screen::Chat(id) => {
                let chat_object = Store::global(cx)
                    .read(cx)
                    .summary(id)
                    .is_some_and(|s| s.type_key == "chat");
                if chat_object {
                    home(id)
                } else {
                    Some(Screen::Object(id.clone()))
                }
            }
        }
    }

    /// A phone screen's content: its header and page.
    fn render_screen(
        &mut self,
        screen: &Screen,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match screen {
            Screen::Spaces => self.render_m_spaces(cx),
            Screen::Home(id) => {
                let space = Store::global(cx).read(cx).space(id).cloned();
                match space.or_else(|| Self::current_space(cx)) {
                    Some(space) => self.render_m_space_home(&space, window, cx),
                    None => self.render_m_spaces(cx),
                }
            }
            Screen::Object(id) => self.render_m_object(id, &Route::Object(id.clone()), window, cx),
            // The full-screen chat brings its own header and keyboard fit.
            Screen::Chat(id) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.page(&Route::Chat(id.clone()), window, cx))
                .into_any_element(),
            Screen::Other => self.render_m_graph(route, window, cx),
        }
    }

    /// Whether the phone screens slide (a touch screen, motion allowed).
    fn mobile_motion(cx: &App) -> bool {
        layout::is_touch() && !motion::reduced(cx)
    }

    /// A newly shown screen: slides in over the screen it was opened from;
    /// anything else (a pop landing, a jump) cancels the motion.
    fn back_observe(&mut self, screen: &Screen, width: f32, cx: &App) {
        let Some(prev) = self.back.shown.replace(screen.clone()) else {
            return;
        };
        if prev == *screen {
            return;
        }
        let push =
            Self::mobile_motion(cx) && Self::mobile_parent(screen, cx).as_ref() == Some(&prev);
        self.back.offset = motion::ValueState::new(if push { f64::from(width) } else { 0. });
        self.back.phase = if push {
            Phase::Pushing(prev)
        } else {
            Phase::Idle
        };
    }

    /// The top screen's offset this frame; a pop that has slid off goes back.
    fn back_offset(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> f32 {
        let spring = motion::release_spring(back_swipe::SETTLE);
        let instant = motion::Transition::Instant;
        let (target, transition) = match &self.back.phase {
            Phase::Idle => (0., instant),
            Phase::Tracking(_) if motion::reduced(cx) => (0., instant),
            Phase::Tracking(track) => (track.offset.min(width), instant),
            Phase::Popping => (width, spring),
            Phase::Popped => (width, instant),
            Phase::Returning | Phase::Pushing(_) => (0., spring),
        };
        let frame = self
            .back
            .offset
            .frame(f64::from(target), transition, window, cx);
        if !frame.is_animating() {
            match self.back.phase {
                Phase::Popping => {
                    self.back.phase = Phase::Popped;
                    cx.defer_in(window, |this, _, cx| this.mobile_pop(cx));
                }
                Phase::Returning | Phase::Pushing(_) => self.back.phase = Phase::Idle,
                _ => {}
            }
        }
        frame.value() as f32
    }

    /// The screen showing under the top one while it moves.
    fn back_under(&self, screen: &Screen, cx: &App) -> Option<Screen> {
        match &self.back.phase {
            Phase::Idle => None,
            Phase::Pushing(from) => Some(from.clone()),
            Phase::Tracking(_) | Phase::Popping | Phase::Popped | Phase::Returning => {
                Self::mobile_parent(screen, cx)
            }
        }
    }

    /// A back button: slide the screen off to its parent, or go at once.
    fn mobile_back(&mut self, cx: &mut Context<Self>) {
        let route = Navigator::global(cx).read(cx).route().clone();
        let screen = self.mobile_screen(&route, cx);
        if Self::mobile_motion(cx)
            && matches!(self.back.phase, Phase::Idle)
            && Self::mobile_parent(&screen, cx).is_some()
        {
            self.back.phase = Phase::Popping;
            cx.notify();
        } else {
            self.mobile_pop(cx);
        }
    }

    /// Show the current screen's parent (`mobileBackToSpace` for an object:
    /// its space home, the pinned-cards frame, not history - one more back
    /// reaches the Spaces list).
    fn mobile_pop(&mut self, cx: &mut Context<Self>) {
        let route = Navigator::global(cx).read(cx).route().clone();
        let screen = self.mobile_screen(&route, cx);
        match Self::mobile_parent(&screen, cx) {
            Some(Screen::Spaces) => self.mobile_space_open = false,
            Some(Screen::Home(space)) => {
                Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&space, cx));
                self.mobile_space_open = true;
                navigate(Route::Home, cx);
            }
            Some(Screen::Object(id)) => navigate(Route::Object(id), cx),
            Some(Screen::Chat(_) | Screen::Other) | None => {}
        }
        cx.notify();
    }

    /// iOS's interactive pop: a pan from the left edge drags the screen
    /// back ([`back_swipe`]). Listens in the capture phase, ahead of the
    /// page's scrollers, and keeps the pans it takes (and their fling).
    fn back_swipe_listener(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity().downgrade();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let here = hitbox.should_handle_scroll(window);
                    let ours = this
                        .update(cx, |this, cx| this.back_pan(e, here, window, cx))
                        .unwrap_or(false);
                    if ours {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// One step of a touch pan: true when it is the back swipe's.
    fn back_pan(
        &mut self,
        e: &ScrollWheelEvent,
        here: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ScrollDelta::Pixels(delta) = e.delta else {
            return false;
        };
        let dx = f32::from(delta.x);
        let now = web_time::Instant::now();
        match e.touch_phase {
            TouchPhase::Started => {
                self.back.swallow = false;
                let route = Navigator::global(cx).read(cx).route().clone();
                let screen = self.mobile_screen(&route, cx);
                let takes = here
                    && matches!(self.back.phase, Phase::Idle)
                    && self.space_lift.is_none()
                    && back_swipe::claims(e.position, delta)
                    && Self::mobile_parent(&screen, cx).is_some();
                if !takes {
                    return false;
                }
                let mut track = Track::default();
                track.push(now, dx);
                self.back.phase = Phase::Tracking(track);
            }
            TouchPhase::Moved => {
                if self.back.swallow {
                    return true;
                }
                let Phase::Tracking(track) = &mut self.back.phase else {
                    return false;
                };
                track.push(now, dx);
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.back.swallow {
                    // The fling's last step.
                    self.back.swallow = false;
                    return true;
                }
                let Phase::Tracking(track) = &mut self.back.phase else {
                    return false;
                };
                track.push(now, dx);
                let width = f32::from(window.viewport_size().width);
                let back = e.touch_phase == TouchPhase::Ended
                    && back_swipe::commits(track.offset, width, track.velocity(now));
                self.back.phase = if back {
                    Phase::Popping
                } else {
                    Phase::Returning
                };
                self.back.swallow = e.touch_phase == TouchPhase::Ended;
            }
        }
        cx.notify();
        true
    }

    /// `.m-main`: the route's page, scrolling under the header.
    fn render_m_main(
        &mut self,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let page = self.page(route, window, cx);
        let scroll = self.scrolls.entry(route.clone()).or_default().clone();
        div()
            .id(("m-main", page.entity_id()))
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .child(self.pull_to_sync(&scroll))
            .child(div().w_full().pt(px(24.)).pb(px(80.)).child(page))
    }

    /// `/app/graph`: back (history) and the Graph title.
    fn render_m_graph(
        &mut self,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                m_top()
                    .child(m_obj(None, "Graph".into()))
                    .child(
                        m_btn("m-back", cx)
                            .child(back_glyph())
                            .accessibility_label("Back")
                            .tip("Back")
                            .on_click(|_, _, cx| {
                                Navigator::global(cx).update(cx, |n, cx| n.back(cx))
                            }),
                    )
                    .child(div().flex_1()),
            )
            .child(self.render_m_main(route, window, cx))
            .into_any_element()
    }

    /// An object: back to its space home, its icon and name, then the sync
    /// dot, ⋯ and the chat & properties button as fit what it is.
    fn render_m_object(
        &mut self,
        id: &str,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (path_icon, path_name, graph) = self.header_path(route, cx);
        let (relation, summary, space) = {
            let store = Store::global(cx).read(cx);
            let relation = store.relations.iter().any(|r| r.id == id);
            let summary = store.summary(id).is_some();
            let space = !summary && store.space(id).is_some();
            (relation, summary, space)
        };
        let (pane_available, count) = {
            let pane = PaneState::global(cx).read(cx);
            (pane.available, pane.count)
        };
        let pane_btn = (pane_available && (summary || space)).then(|| {
            Badge::new()
                .count(count)
                .max(99)
                .color(theme::accent())
                .child(
                    m_btn("m-pane", cx)
                        .child(Icon::new(Glyph::Pane).size(px(18.)))
                        .accessibility_label("Chat & properties")
                        .tip("Chat & properties")
                        .on_click(|_, _, cx| {
                            PaneState::global(cx).update(cx, |p, cx| p.set_sheet(true, cx))
                        }),
                )
        });
        let sync = (relation || summary).then(|| self.render_m_sync(window, cx));
        let actions = (relation || summary || space).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .children(sync)
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            m_btn("m-more", cx)
                                .child(div().text_size(px(19.)).child("⋯"))
                                .accessibility_label("More")
                                .tip("More")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.toggle_more(window, cx)),
                                ),
                        )
                        .child(self.anchor_probe("more")),
                )
                .children(pane_btn)
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                m_top()
                    .child(m_obj((!graph).then_some(path_icon), path_name))
                    .child(
                        m_btn("m-back", cx)
                            .child(back_glyph())
                            .accessibility_label("Space home")
                            .tip("Space home")
                            .on_click(cx.listener(|this, _, _, cx| this.mobile_back(cx))),
                    )
                    .children(actions),
            )
            .child(self.render_m_main(route, window, cx))
            .into_any_element()
    }

    /// `.m-sync`: the sync dot as a button - green when synced, a pulsing
    /// orange while busy, a progress ring (filled to the pass's percentage,
    /// or a spinning arc) and its label while a history pass runs. A tap
    /// syncs now where the host can. The dot cross-fades between states and
    /// the ring and its count ease to each new value (Svelte jumped), each
    /// retargeting when the pass reports faster than it eases.
    fn render_m_sync(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let status = Store::global(cx).read(cx).status.clone();
        let pending = status.pending.unwrap_or(0);
        let ok = status.phase == SyncPhase::Live && pending == 0;
        let busy = status.phase == SyncPhase::Backfill || pending > 0;
        let progress = sync_progress(&status);
        let pct = progress.and_then(|p| p.pct);
        let shown_pct =
            pct.map(|pct| motion::animate("m-sync-pct", pct as f32, motion::CONTROL, window, cx));
        let shown_received = progress.filter(|p| p.pct.is_none()).map(|p| {
            let received = p.received as f32;
            motion::animate("m-sync-received", received, motion::CONTROL, window, cx)
        });
        let dot_color = if ok {
            theme::green()
        } else if busy {
            theme::orange()
        } else {
            theme::muted()
        };
        let dot_color = list_motion::blend(
            "m-sync-dot",
            dot_color,
            motion::Tween::ease(200),
            window,
            cx,
        );
        let dot = div().flex_none().size(px(12.)).rounded_full().bg(dot_color);
        let dot = if busy && pct.is_none() {
            dot.with_animation(
                "m-sync-pulse",
                Animation::new(Duration::from_millis(1200))
                    .repeat()
                    .with_easing(ease_in_out),
                |d, t| d.opacity(1. - 0.65 * (1. - (2. * t - 1.).abs())),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        };
        // The 20px ring around the dot: filled to the pass's percentage (the
        // kit eases it to each new value), or an indeterminate arc.
        let ring = progress.map(|_| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    ProgressCircle::new("m-sync-ring")
                        .color(theme::orange())
                        .loading(pct.is_none())
                        .value(pct.map_or(0., |pct| pct as f32))
                        .size(px(20.)),
                )
        });
        let label = progress.map(|p| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(1.))
                .flex()
                .justify_center()
                .text_size(px(8.))
                .line_height(px(8.))
                .text_color(theme::muted())
                .child(match shown_pct {
                    Some(pct) => format!("{}%", pct.round() as u64),
                    None => count(shown_received.map_or(p.received, |r| r.round() as u64)),
                })
        });
        let sync = sync_now(cx);
        panel_button("m-sync", cx)
            .debug_selector(|| "m-sync".into())
            .size(px(40.))
            .rounded(px(20.))
            .border_0()
            .accessibility_label("Sync")
            .tip(sync_tip(&status))
            .map(|b| match sync {
                Some(sync) => b.on_click(move |_, _, cx| (sync.0)(cx)),
                None => b.cursor_default(),
            })
            .children(ring)
            .child(dot)
            .children(label)
            .into_any_element()
    }

    /// A canvas arming pull-to-sync on `scroll`: a touch pan that starts on
    /// it (not on an overlay above it) and pulls down while it already sits
    /// at its top, by more than [`PULL_TO_SYNC`], syncs now when released.
    fn pull_to_sync(&self, scroll: &ScrollHandle) -> impl IntoElement {
        let pull = self.pull.clone();
        let area = scroll.clone();
        let scroll = scroll.clone();
        canvas(
            move |_, window, _| window.insert_hitbox(area.bounds(), HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let pull = pull.clone();
                let scroll = scroll.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    // Capture: the offset before this step scrolls the list.
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let ScrollDelta::Pixels(delta) = e.delta else {
                        return;
                    };
                    let at_top = scroll.offset().y >= px(0.);
                    let dy = f32::from(delta.y);
                    match e.touch_phase {
                        TouchPhase::Started => {
                            let here = hitbox.should_handle_scroll(window);
                            pull.set((here && at_top).then_some(dy));
                        }
                        TouchPhase::Moved => {
                            if let Some(pulled) = pull.get() {
                                pull.set(at_top.then_some(pulled + dy));
                            }
                        }
                        TouchPhase::Ended => {
                            if let Some(pulled) = pull.take()
                                && pulled + dy > PULL_TO_SYNC
                                && let Some(sync) = sync_now(cx)
                            {
                                (sync.0)(cx);
                            }
                        }
                        TouchPhase::Cancelled => pull.set(None),
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// Screen 1: the Spaces list (avatar → Settings, the cards in this
    /// person's order, held to reorder, New space, the build stamp).
    fn render_m_spaces(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (spaces, latest, status) = {
            let store = Store::global(cx).read(cx);
            let spaces: Vec<Space> = store.ordered_spaces().into_iter().cloned().collect();
            // `latestInChannel`: each space's most recently touched object.
            let latest: Vec<Option<ObjectSummary>> = spaces
                .iter()
                .map(|space| {
                    store
                        .summaries
                        .iter()
                        .filter(|s| s.channel_id == space.id)
                        .fold(None::<&ObjectSummary>, |best, s| match best {
                            Some(b) if s.updated_at <= b.updated_at => Some(b),
                            _ => Some(s),
                        })
                        .cloned()
                })
                .collect();
            (spaces, latest, store.status.clone())
        };
        let lift = self.space_lift.clone();
        let over = self.space_over.clone().filter(|_| lift.is_some());
        self.card_bounds.borrow_mut().clear();

        let mut cards = div()
            .id("m-cards")
            .debug_selector(|| "m-cards".into())
            .relative()
            .flex()
            .flex_col()
            .gap(px(10.))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.mobile_spaces_scroll)
            .pb(px(12.))
            .child(self.pull_to_sync(&self.mobile_spaces_scroll))
            .child(self.space_lift_listener(cx));
        for (space, latest) in spaces.into_iter().zip(latest) {
            let id = space.id.clone();
            let lifted = lift.as_deref() == Some(&id);
            let over_here = over
                .as_ref()
                .filter(|(o, _)| *o == id)
                .map(|(_, after)| *after);
            let bounds = self.card_bounds.clone();
            let probe_id = id.clone();
            cards = cards.child(
                div()
                    .id(crate::eid("m-card", &id))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("m-card-{id}")
                    })
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(12.))
                    .bg(theme::panel())
                    .border_1()
                    .border_color(if lifted {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .rounded(px(10.))
                    .p(px(14.))
                    .min_h(px(64.))
                    .text_color(theme::fg())
                    .cursor_pointer()
                    .when(lifted, |d| {
                        d.shadow(vec![BoxShadow {
                            color: hsla(0., 0., 0., 0.45),
                            offset: point(px(0.), px(10.)),
                            blur_radius: px(28.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                    })
                    .child(
                        canvas(
                            move |b, _, _| bounds.borrow_mut().push((probe_id.clone(), b)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .size(px(44.))
                            .rounded(px(10.))
                            .bg(theme::hover())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(22.))
                            .overflow_hidden()
                            .child(ui::icon_text(&space_glyph(&space), px(44.))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(space.name.clone()),
                            )
                            .when_some(latest.as_ref(), |d, latest| {
                                let icon = if latest.icon.is_empty() {
                                    String::new()
                                } else {
                                    format!("{} ", latest.icon)
                                };
                                let name = if latest.name.is_empty() {
                                    "Untitled"
                                } else {
                                    &latest.name
                                };
                                d.child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme::muted())
                                        .truncate()
                                        .child(format!("{icon}{name}")),
                                )
                            }),
                    )
                    .when_some(latest.as_ref(), |d, latest| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(short_date(latest.updated_at)),
                        )
                    })
                    .when_some(over_here, |d, after| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(8.))
                                .right(px(8.))
                                .h(px(2.))
                                .rounded(px(2.))
                                .bg(theme::accent())
                                .map(|l| {
                                    if after {
                                        l.bottom(px(-5.))
                                    } else {
                                        l.top(px(-5.))
                                    }
                                }),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // A completed lift is a reorder, never navigation.
                        if this.space_lift.is_some() {
                            return;
                        }
                        Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&id, cx));
                        this.mobile_space_open = true;
                        cx.notify();
                    })),
            );
        }
        cards = cards.child(
            panel_button("m-new-space", cx)
                .w_full()
                .h_auto()
                .p(px(12.))
                .rounded(px(10.))
                .text_color(theme::muted())
                .child(div().text_size(px(16.)).child("＋ New space"))
                .on_click(cx.listener(|this, _, window, cx| this.new_space(window, cx))),
        );

        let importing = (!status.bootstrapped && status.phase != SyncPhase::Idle).then(|| {
            let detail = status
                .detail
                .clone()
                .unwrap_or_else(|| format!("{} changes imported", count(status.imported)));
            Alert::info(
                "m-importing",
                format!("Loading your vault… {detail}. Spaces appear as they arrive."),
            )
            .w_auto()
            .mx(px(16.))
            .mb(px(8.))
            .p_0()
            .gap(px(6.))
            .border_0()
            .bg(transparent_black())
            .text_size(px(12.))
            .line_height(relative(1.35))
            .text_color(theme::muted())
        });
        div()
            .debug_selector(|| "m-spaces".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(12.))
            .px(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .mb(px(14.))
                    .child(
                        div().flex().justify_end().pb(px(10.)).child(
                            crate::fill_button("m-avatar", transparent_black(), cx)
                                .size(px(36.))
                                .rounded(px(18.))
                                .child(
                                    crate::rail::account_avatar(px(36.), Glyph::Settings, cx)
                                        .bg(theme::hover())
                                        .border_color(theme::border())
                                        .text_color(theme::fg()),
                                )
                                .accessibility_label("Settings")
                                .tip("Settings")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.settings(window, cx)),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .child(ui::spaced_text("Spaces", -0.01)),
                    ),
            )
            .children(importing)
            .child(cards)
            .child(
                div()
                    .mb(px(4.))
                    .flex()
                    .justify_center()
                    .text_size(px(10.))
                    .text_color(theme::muted())
                    .opacity(0.6)
                    .child(format!("Build {BUILD_STAMP}")),
            )
            .into_any_element()
    }

    /// Holding a space card lifts it (the platform's long press); moving
    /// the finger marks where it drops, releasing commits the move.
    fn space_lift_listener(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let cards = self.card_bounds.clone();
        let list = self.mobile_spaces_scroll.clone();
        canvas(
            move |_, window, _| window.insert_hitbox(list.bounds(), HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let this = this.clone();
                let cards = cards.clone();
                window.on_mouse_event(move |e: &LongPressEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some(entity) = this.upgrade() else {
                        return;
                    };
                    let hit = |p: Point<Pixels>| {
                        cards
                            .borrow()
                            .iter()
                            .find(|(_, b)| b.contains(&p))
                            .map(|(id, b)| (id.clone(), p.y > b.center().y))
                    };
                    match e.phase {
                        TouchPhase::Started => {
                            // Not through an overlay above the list.
                            if !hitbox.is_hovered(window) {
                                return;
                            }
                            let Some((id, _)) = hit(e.start_position) else {
                                return;
                            };
                            window.prevent_default();
                            window.capture_long_press(&entity);
                            cx.stop_propagation();
                            entity.update(cx, |this, cx| {
                                this.space_lift = Some(id);
                                this.space_over = None;
                                cx.notify();
                            });
                        }
                        TouchPhase::Moved => {
                            let target = hit(e.position);
                            entity.update(cx, |this, cx| {
                                let Some(lift) = this.space_lift.clone() else {
                                    return;
                                };
                                if let Some((id, after)) = target
                                    && id != lift
                                    && this.space_over.as_ref() != Some(&(id.clone(), after))
                                {
                                    this.space_over = Some((id, after));
                                    cx.notify();
                                }
                            });
                        }
                        TouchPhase::Ended | TouchPhase::Cancelled => {
                            entity.update(cx, |this, cx| {
                                let Some(lift) = this.space_lift.take() else {
                                    return;
                                };
                                if let Some((target, after)) = this.space_over.take() {
                                    commit_space_move(&lift, &target, after, cx);
                                }
                                cx.notify();
                            });
                        }
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    fn flip_mobile(&mut self, key: &str, cx: &mut Context<Self>) {
        let collapsed = !self.mobile_collapsed.get(key).copied().unwrap_or(false);
        self.mobile_collapsed.insert(key.to_string(), collapsed);
        cx.notify();
    }

    /// `.m-section`: an uppercase label and ⌄ that fold it (with `add`
    /// between them while open), over a card of rows.
    fn m_section(
        &self,
        key: &'static str,
        label: &'static str,
        add: Option<AnyElement>,
        rows: Vec<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = !self.mobile_collapsed.get(key).copied().unwrap_or(false);
        // The body folds with the ⌄; a long section only fades.
        let fold = list_motion::fold(
            crate::eid("m-section-fold", key),
            open,
            Some(motion::FADE),
            rows.len() <= 30,
            window,
            cx,
        );
        let body = GroupBox::new()
            .id(crate::eid("m-section-box", key))
            .mb(px(6.))
            .content_style(
                StyleRefinement::default()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(10.))
                    .pt(px(4.))
                    .px(px(10.))
                    .pb(px(8.))
                    .gap_0(),
            )
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                    .child(row)
            }));
        div()
            .flex()
            .flex_none()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id(crate::eid("m-section", key))
                            .flex_1()
                            .flex()
                            .items_center()
                            .pt(px(10.))
                            .px(px(4.))
                            .pb(px(6.))
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .child(ui::spaced_text(label.to_uppercase(), 0.05))
                            .on_click(cx.listener(move |this, _, _, cx| this.flip_mobile(key, cx))),
                    )
                    .when(open, |d| d.children(add))
                    .child(
                        m_chev(crate::eid("m-section-chev", key), fold.turn(), cx)
                            .on_click(cx.listener(move |this, _, _, cx| this.flip_mobile(key, cx))),
                    ),
            )
            .when(fold.shown(), |d| d.child(fold.wrap(body)))
    }

    /// `.m-section-add`: a 44px ＋ target.
    fn m_section_add(id: &'static str, label: &'static str, cx: &App) -> Button {
        crate::fill_button(id, transparent_black(), cx)
            .relative()
            .size(px(44.))
            .child(Icon::new(Glyph::Plus).size(px(17.)))
            .accessibility_label(label)
    }

    /// Screen 2: a space's home - its pinned objects as cards, Recently
    /// edited, Types (only creatable here on a phone: the bottom ＋ makes
    /// objects), Properties, and the bottom bar (Search, ＋ New object).
    fn render_m_space_home(
        &mut self,
        current: &Space,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = Navigator::global(cx).read(cx).space().to_string();
        let space_id = Self::space_id(cx);

        // Pinned cards.
        let pinned: Vec<ObjectSummary> = {
            let store = Store::global(cx).read(cx);
            current
                .pinned_ids
                .iter()
                .filter_map(|id| store.summary(id).cloned())
                .collect()
        };
        let card_width = f32::from(window.viewport_size().width) - 28.;
        let mut column = div()
            .id("m-cards-col")
            .relative()
            .flex()
            .flex_col()
            .gap(px(10.))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.mobile_home_scroll)
            .pb(px(12.))
            .child(self.pull_to_sync(&self.mobile_home_scroll))
            // Lite: the vault or this space not ready yet, or the space loading.
            .children(
                roostr_core::lite_ui::ready_notice(&current.id, cx)
                    .map(|notice| div().px(px(14.)).child(notice)),
            );
        let pins = list_motion::pins("m-pins", &current.id, &current.pinned_ids, window, cx);
        for p in pinned {
            let id = p.id.clone();
            let collapsed = self.pin_collapsed(&id, cx);
            // Only this device's own fold moves (`.m-wcard-body` popped in
            // Svelte); one sync brings in snaps.
            let own = self.pin_folds.contains_key(&id);
            let fold = list_motion::fold(
                crate::eid("m-wcard-fold", &id),
                !collapsed,
                own.then_some(motion::FADE),
                true,
                window,
                cx,
            );
            let icon = object_icon(Store::global(cx).read(cx), &p.icon, &p.type_key, &active);
            let name = if p.name.is_empty() {
                "Untitled".to_string()
            } else {
                p.name.clone()
            };
            let body = fold
                .shown()
                .then(|| {
                    let view = self
                        .pinned_views
                        .entry(id.clone())
                        .or_insert_with(|| cx.new(|cx| PinnedWidget::new(id.clone(), cx)))
                        .clone();
                    // The gallery fits its columns to the card's inner width.
                    view.update(cx, |w, _| w.side_width = card_width - 2. - 20. + 48.);
                    view
                })
                // A widget with no preview (notes, pages) has no body at all.
                .filter(|view| !view.read(cx).is_empty());
            let open_id = id.clone();
            let flip_id = id.clone();
            let card = GroupBox::new()
                .id(crate::eid("m-wcard-box", &id))
                .flex_none()
                .content_style(
                    StyleRefinement::default()
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(10.))
                        .overflow_hidden()
                        .gap_0(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .child(
                            crate::fill_button(crate::eid("m-wcard", &id), transparent_black(), cx)
                                .flex_1()
                                .min_w_0()
                                .h_auto()
                                .pt(px(12.))
                                .pr(px(6.))
                                .pb(px(12.))
                                .pl(px(16.))
                                .text_color(theme::fg())
                                .accessibility_label(name.clone())
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .justify_start()
                                        .gap(px(10.))
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .line_height(relative(1.2))
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(22.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(16.))
                                                .line_height(px(16.))
                                                .text_color(theme::accent())
                                                .child(ui::icon_text(&icon, px(16.))),
                                        )
                                        .child(div().flex_1().min_w_0().truncate().child(name)),
                                )
                                .on_click(move |_, _, cx| {
                                    navigate(Route::Object(open_id.clone()), cx)
                                }),
                        )
                        .child(
                            m_chev(crate::eid("m-wcard-chev", &id), fold.turn(), cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_pin_collapsed(&flip_id, cx)
                                }),
                            ),
                        ),
                )
                .children(body.map(|view| fold.wrap(div().px(px(10.)).pb(px(10.)).child(view))));
            column = column.child(pins.card(&id, card, window, cx));
        }

        // Recently edited: the space's newest six, pinned or not.
        let recent_rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            let mut recents: Vec<&ObjectSummary> = store
                .summaries
                .iter()
                .filter(|s| s.channel_id == current.id)
                .collect();
            recents.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
            recents
                .into_iter()
                .take(6)
                .map(|o| {
                    let name = if o.name.is_empty() {
                        "Untitled".to_string()
                    } else {
                        o.name.clone()
                    };
                    let lead = if store.layout_of(&o.type_key) == "task" {
                        let id = o.id.clone();
                        ui::checkbox(crate::eid("m-row-check", &o.id), o.done == Some(true))
                            .flex_none()
                            .mr(px(6.))
                            .on_change(move |done: &bool, _, cx| {
                                cx.stop_propagation();
                                crate::sidebar::set_done(&id, *done, cx);
                            })
                            .into_any_element()
                    } else {
                        let icon = if o.icon.is_empty() {
                            type_glyph(store, &o.type_key, &active)
                        } else {
                            o.icon.clone()
                        };
                        row_icon(&icon).into_any_element()
                    };
                    let id = o.id.clone();
                    m_row(crate::eid("m-recent", &o.id), lead, name, None)
                        .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                        .into_any_element()
                })
                .collect()
        };
        let recent = self.m_section("__recent", "Recently edited", None, recent_rows, window, cx);
        column = column.child(pins.follow("section:recent", recent));

        // Types.
        let type_rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            store
                .types
                .iter()
                .filter(|t| t.space == space_id)
                .map(|t| {
                    let icon = if t.icon.is_empty() {
                        type_glyph(store, &t.key, &active)
                    } else {
                        t.icon.clone()
                    };
                    let name = if t.name.is_empty() {
                        t.key.clone()
                    } else {
                        t.name.clone()
                    };
                    let id = t.id.clone();
                    let mark = t
                        .builtin
                        .then(|| builtin_mark(crate::eid("m-type-lock", &t.id)));
                    m_row(
                        crate::eid("m-type", &t.id),
                        row_icon(&icon).into_any_element(),
                        name,
                        mark,
                    )
                    .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                    .into_any_element()
                })
                .collect()
        };
        let add_type = Self::m_section_add("m-new-type", "New type", cx)
            .on_click(cx.listener(|this, _, window, cx| this.new_type(window, cx)))
            .into_any_element();
        let types = self.m_section("__types", "Types", Some(add_type), type_rows, window, cx);
        column = column.child(pins.follow("section:types", types));

        // Properties.
        let prop_rows: Vec<AnyElement> = {
            let store = Store::global(cx).read(cx);
            let mut props: Vec<&RelationDef> = space_relations(&store.relations, &space_id)
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
            props
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
                    let mark = store
                        .builtin_ids
                        .contains(&r.id)
                        .then(|| builtin_mark(crate::eid("m-prop-lock", &r.id)));
                    m_row(
                        crate::eid("m-prop", &r.id),
                        row_icon(&icon).into_any_element(),
                        name,
                        mark,
                    )
                    .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                    .into_any_element()
                })
                .collect()
        };
        let add_prop = Self::m_section_add("m-new-prop", "New property", cx)
            .child(self.anchor_probe("new-prop"))
            .on_click(cx.listener(|this, _, window, cx| this.open_new_property(window, cx)))
            .into_any_element();
        let props = self.m_section(
            "__props",
            "Properties",
            Some(add_prop),
            prop_rows,
            window,
            cx,
        );
        column = column.child(pins.follow("section:props", props));

        let settings_id = current.id.clone();
        div()
            .debug_selector(|| "m-space-home".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(12.))
            .px(px(14.))
            .child(
                // Pulled back to the screen edges so every back button sits in one place.
                m_top()
                    .mt(px(-12.))
                    .mx(px(-14.))
                    .child(
                        m_btn("m-spaces", cx)
                            .child(back_glyph())
                            .accessibility_label("Spaces")
                            .tip("Spaces")
                            .on_click(cx.listener(|this, _, _, cx| this.mobile_back(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        m_btn("m-space-settings", cx)
                            .child(Icon::new(Glyph::Settings).size(px(18.)))
                            .accessibility_label("Space settings")
                            .tip("Space settings")
                            .on_click(move |_, _, cx| {
                                navigate(Route::Object(settings_id.clone()), cx)
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pt(px(8.))
                    .px(px(4.))
                    .pb(px(16.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(52.))
                            .rounded(px(12.))
                            .bg(theme::hover())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(26.))
                            .overflow_hidden()
                            .child(ui::icon_text(&space_glyph(current), px(52.))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(current.name.clone()),
                            )
                            .child(div().text_size(px(13.)).text_color(theme::muted()).child(
                                if current.members.is_empty() {
                                    "Private space"
                                } else {
                                    "Shared space"
                                },
                            )),
                    ),
            )
            .child(column)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(10.))
                    .pt(px(10.))
                    .pb(px(12.))
                    .child(
                        panel_button("m-search", cx)
                            .debug_selector(|| "m-search".into())
                            .flex_1()
                            .h_auto()
                            .rounded(px(999.))
                            .text_color(theme::muted())
                            .px(px(16.))
                            .py(px(12.))
                            .accessibility_label("Search")
                            .child(div().w_full().text_size(px(15.)).child("⌕ Search"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_search(window, cx)),
                            ),
                    )
                    .child(
                        panel_button("m-compose", cx)
                            .debug_selector(|| "m-compose".into())
                            .size(px(46.))
                            .rounded(px(23.))
                            .child(Icon::new(Glyph::Plus).size(px(22.)))
                            .accessibility_label("New object")
                            .tip("New object")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.create_menu.is_some() {
                                    this.create_menu = None;
                                    cx.notify();
                                } else {
                                    this.open_create_menu(window, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// `.m-create-menu`: the New menu docked above the bottom bar, 14px
    /// from the sides, scrolling inside itself below the top of the screen.
    /// It rises 4px into place as it fades in (input from its first frame).
    pub(crate) fn render_m_create_menu(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let menu = self.create_menu.clone()?;
        let max_h = roostr_core::layout::viewport(window).height - px(96.);
        let appear = roostr_core::motion::appear_frame("m-create-menu-appear", 4., window, cx);
        Some(
            div()
                .debug_selector(|| "m-create-menu".into())
                .absolute()
                .left(px(14.))
                .right(px(14.))
                .bottom(px(72.))
                .max_h(max_h)
                .flex()
                .flex_col()
                .child(appear.apply(div().min_h_0().flex().flex_col().child(menu)))
                .into_any_element(),
        )
    }

    /// Phones show no tab strip: entering a space opens its home (`path &&
    /// !isMobile`) - the graph stays, it is per space.
    pub(crate) fn mobile_space_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !layout::is_mobile(window) {
            return;
        }
        let target = match &self.shown_route {
            Route::Graph { .. } => self.shown_route.clone(),
            _ => Route::Home,
        };
        navigate(target, cx);
    }
}
