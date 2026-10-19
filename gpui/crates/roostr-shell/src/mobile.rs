//! The mobile shell (`+layout.svelte` `{#if isMobile}` markup 911-1238, CSS
//! 2846-3461): at [`layout::is_mobile`] widths the app is the Anytype iOS
//! flow - Spaces list → space home → object - instead of rail | sidebar |
//! main | pane. Every `env(safe-area-inset-*)` term is 0: each host lays the
//! window out inside the safe area and above the keyboard, as the iOS app's
//! web view sat.

use std::time::Duration;

use chrono::{Local, TimeZone as _};
use gpui_kit::component::IconNamed as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::layout::{self, sync_now};
use roostr_core::nav::{Navigator, Route, navigate};
use roostr_core::objects::{is_image_icon, object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::store::Store;
use roostr_core::sync_status::{count, sync_progress, sync_tip};
use roostr_core::types::{ObjectSummary, RelationDef, Space, SyncPhase, Value};
use roostr_core::{Glyph, PaneState, theme, ui};

use crate::PaneHost;
use crate::pinned::PinnedWidget;
use crate::rail::commit_space_move;
use crate::tabs::checkbox_icon;
use crate::workspace::Workspace;

/// The web app's `__BUILD_STAMP__` (see `build.rs`).
const BUILD_STAMP: &str = env!("ROOSTR_BUILD_STAMP");

/// Pulling a page scroller down this far at its top and letting go syncs now
/// (the iOS web view's pull-to-refresh, which called `Backend.resume`).
const PULL_TO_SYNC: f32 = 64.;

/// `.m-btn`: a 40px round panel button.
fn m_btn(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .size(px(40.))
        .rounded_full()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .text_color(theme::fg())
        .text_size(px(19.))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
}

/// The mobile headers' back chevron (20px).
fn back_glyph() -> Svg {
    svg()
        .path(Glyph::MobileBack.path())
        .size(px(20.))
        .flex_none()
        .text_color(theme::fg())
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

/// `.m-chev`: the ⌄ of a collapsible card or section, turned over while open.
fn m_chev(id: impl Into<ElementId>, open: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .px(px(14.))
        .py(px(12.))
        .cursor_pointer()
        .child(
            svg()
                .path(Glyph::ChevronDown.path())
                .size(px(13.))
                .text_color(theme::muted())
                .when(open, |s| {
                    s.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)))
                }),
        )
}

/// `.m-row`: a row of a space-home section.
fn m_row(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(9.))
        .px(px(6.))
        .rounded(theme::radius::md())
        .text_size(px(14.))
        .text_color(theme::fg())
        .cursor_pointer()
        .active(|s| s.bg(theme::hover()))
}

/// `.obj-icon` at its 20px width.
fn row_icon(icon: &str) -> Div {
    crate::sidebar::obj_icon(icon)
}

/// `.builtin-mark`: an 11px lock pushed to the row's end.
fn builtin_mark(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .ml_auto()
        .pl(px(6.))
        .flex()
        .flex_none()
        .opacity(0.55)
        .child(
            svg()
                .path(Glyph::Lock.path())
                .size(px(11.))
                .text_color(theme::muted()),
        )
        .tooltip(crate::tip("Roostr default - can't be deleted"))
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

/// The conic ring around the sync dot: `arc` of a 20px ring (2px thick)
/// from `start` (fractions of a turn, 0 = 12 o'clock) in orange, the rest
/// in the border colour.
fn paint_sync_ring(bounds: Bounds<Pixels>, start: f32, arc: f32, window: &mut Window) {
    let center = bounds.center();
    let ring = |from: f32, to: f32, color: Hsla, window: &mut Window| {
        if to <= from {
            return;
        }
        let mut path = PathBuilder::stroke(px(2.));
        let steps = 48;
        for i in 0..=steps {
            let turn = from + (to - from) * i as f32 / steps as f32;
            let angle = std::f32::consts::TAU * turn - std::f32::consts::FRAC_PI_2;
            let p = point(
                center.x + px(9.) * angle.cos(),
                center.y + px(9.) * angle.sin(),
            );
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    };
    ring(start + arc, start + 1., theme::border(), window);
    ring(start, start + arc, theme::orange(), window);
}

impl Workspace {
    /// `.m-shell`: the screen for the route at a phone width.
    pub(crate) fn render_mobile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let route = Navigator::global(cx).read(cx).route().clone();
        // The full-screen chat brings its own header and keyboard fit.
        if matches!(route, Route::Chat(_)) {
            return self.page(&route, window, cx).into_any_element();
        }
        let body = match &route {
            Route::Graph { .. } => self.render_m_graph(&route, window, cx),
            Route::Object(id) => self.render_m_object(id, &route, window, cx),
            _ => {
                if self.mobile_space_open
                    && let Some(current) = Self::current_space(cx)
                {
                    self.render_m_space_home(&current, window, cx)
                } else {
                    self.render_m_spaces(cx)
                }
            }
        };
        // On a phone the chat & properties pane is a full-screen sheet over
        // the object (`.disc-drawer` @media 720).
        let pane = PaneState::global(cx).read(cx);
        let (pane_open, pane_available) = (pane.open, pane.available);
        let sheet = (matches!(route, Route::Object(_)) && pane_open && pane_available)
            .then(|| PaneHost::global(cx).read(cx).view.clone())
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
            .bg(theme::bg())
            .text_color(theme::fg())
            .line_height(relative(1.2))
            .child(body)
            .children(sheet)
            .into_any_element()
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
                        m_btn("m-back")
                            .debug_selector(|| "m-back".into())
                            .child(back_glyph())
                            .map(|d| self.data_tip(d, "tip-m-back", "Back", cx))
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
        let (path_icon, path_name, graph) = self.header_path(cx);
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
            m_btn("m-pane")
                .debug_selector(|| "m-pane".into())
                .child(
                    svg()
                        .path(Glyph::Pane.path())
                        .size(px(18.))
                        .text_color(theme::fg()),
                )
                .when(count > 0, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-4.))
                            .right(px(-4.))
                            .min_w(px(16.))
                            .h(px(16.))
                            .px(px(4.))
                            .rounded(px(8.))
                            .bg(theme::accent())
                            .text_color(theme::white())
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(16.))
                            .flex()
                            .justify_center()
                            .child(if count > 99 {
                                "99+".to_string()
                            } else {
                                count.to_string()
                            }),
                    )
                })
                .map(|d| self.data_tip(d, "tip-m-pane", "Chat & properties", cx))
                .on_click(|_, _, cx| PaneState::global(cx).update(cx, |p, cx| p.set_open(true, cx)))
        });
        let actions = (relation || summary || space).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .when(relation || summary, |d| d.child(self.render_m_sync(cx)))
                .child(
                    div().relative().child(
                        m_btn("m-more")
                            .debug_selector(|| "m-more".into())
                            .child("⋯")
                            .child(self.anchor_probe("more"))
                            .map(|d| self.data_tip(d, "tip-more", "More", cx))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_more(cx))),
                    ),
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
                        m_btn("m-back")
                            .debug_selector(|| "m-back".into())
                            .child(back_glyph())
                            .map(|d| self.data_tip(d, "tip-m-back", "Space home", cx))
                            .on_click(cx.listener(|this, _, _, cx| this.mobile_back_to_space(cx))),
                    )
                    .children(actions),
            )
            .child(self.render_m_main(route, window, cx))
            .into_any_element()
    }

    /// `mobileBackToSpace`: to the object's space home (the pinned-cards
    /// frame), not history - one more tap reaches the Spaces list.
    fn mobile_back_to_space(&mut self, cx: &mut Context<Self>) {
        let owning = Self::open_object_id(cx).and_then(|id| {
            let store = Store::global(cx).read(cx);
            let channel = store.summary(&id)?.channel_id.clone();
            store.space(&channel).map(|s| s.id.clone())
        });
        if let Some(space) = owning {
            Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&space, cx));
        }
        self.mobile_space_open = true;
        navigate(Route::Home, cx);
        cx.notify();
    }

    /// `.m-sync`: the sync dot as a button - green when synced, a pulsing
    /// orange while busy, a progress ring (filled to the pass's percentage,
    /// or a spinning arc) and its label while a history pass runs. A tap
    /// syncs now where the host can.
    fn render_m_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = Store::global(cx).read(cx).status.clone();
        let pending = status.pending.unwrap_or(0);
        let ok = status.phase == SyncPhase::Live && pending == 0;
        let busy = status.phase == SyncPhase::Backfill || pending > 0;
        let progress = sync_progress(&status);
        let pct = progress.and_then(|p| p.pct);
        let dot_color = if ok {
            theme::green()
        } else if busy {
            theme::orange()
        } else {
            theme::muted()
        };
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
        let ring = match (progress, pct) {
            (Some(_), Some(pct)) => Some(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_sync_ring(bounds, 0., pct as f32 / 100., window)
                    },
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            ),
            (Some(_), None) => Some(
                div()
                    .absolute()
                    .inset_0()
                    .with_animation(
                        "m-sync-spin",
                        Animation::new(Duration::from_millis(1600)).repeat(),
                        |d, t| {
                            d.child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, _, window, _| {
                                        paint_sync_ring(bounds, t, 0.25, window)
                                    },
                                )
                                .size_full(),
                            )
                        },
                    )
                    .into_any_element(),
            ),
            (None, _) => None,
        };
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
                .child(match p.pct {
                    Some(pct) => format!("{pct}%"),
                    None => count(p.received),
                })
        });
        let sync = sync_now(cx);
        div()
            .id("m-sync")
            .debug_selector(|| "m-sync".into())
            .relative()
            .flex_none()
            .size(px(40.))
            .rounded_full()
            .bg(theme::panel())
            .flex()
            .items_center()
            .justify_center()
            .tooltip(crate::tip(sync_tip(&status)))
            .when_some(sync, |d, sync| {
                d.cursor_pointer().on_click(move |_, _, cx| (sync.0)(cx))
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
        let (pic, avatar) = {
            let profile = roostr_core::profile::ProfileCache::global(cx).read(cx);
            (profile.picture().to_string(), profile.avatar())
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
            div()
                .id("m-new-space")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(px(10.))
                .p(px(12.))
                .text_color(theme::muted())
                .cursor_pointer()
                .child("＋ New space")
                .on_click(cx.listener(|this, _, window, cx| this.new_space(window, cx))),
        );

        let importing = (!status.bootstrapped && status.phase != SyncPhase::Idle).then(|| {
            let detail = status
                .detail
                .clone()
                .unwrap_or_else(|| format!("{} changes imported", count(status.imported)));
            div()
                .mx(px(16.))
                .mb(px(8.))
                .text_size(px(12.))
                .line_height(relative(1.35))
                .text_color(theme::muted())
                .child(format!(
                    "Loading your vault… {detail}. Spaces appear as they arrive."
                ))
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
                            div()
                                .id("m-avatar")
                                .size(px(36.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(theme::hover())
                                .border_1()
                                .border_color(theme::border())
                                .text_color(theme::fg())
                                .text_size(px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .map(|d| match (avatar, pic.is_empty()) {
                                    (Some(image), _) => d.child(
                                        img(ImageSource::Image(image))
                                            .size_full()
                                            .rounded_full()
                                            .object_fit(ObjectFit::Cover),
                                    ),
                                    (None, false) => d.child(
                                        img(SharedString::from(pic))
                                            .size_full()
                                            .rounded_full()
                                            .object_fit(ObjectFit::Cover),
                                    ),
                                    (None, true) => d.child("⚙"),
                                })
                                .map(|d| self.data_tip(d, "tip-m-avatar", "Settings", cx))
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
        cx: &mut Context<Self>,
    ) -> Div {
        let open = !self.mobile_collapsed.get(key).copied().unwrap_or(false);
        let mut body = div()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .pt(px(4.))
            .px(px(10.))
            .pb(px(8.))
            .mb(px(6.));
        for (i, row) in rows.into_iter().enumerate() {
            body = body.child(
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                    .child(row),
            );
        }
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
                        m_chev(crate::eid("m-section-chev", key), open)
                            .on_click(cx.listener(move |this, _, _, cx| this.flip_mobile(key, cx))),
                    ),
            )
            .when(open, |d| d.child(body))
    }

    /// `.m-section-add`: a 44px ＋ target.
    fn m_section_add(id: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .relative()
            .flex_none()
            .min_w(px(44.))
            .min_h(px(44.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted())
            .text_size(px(17.))
            .line_height(px(17.))
            .cursor_pointer()
            .child("＋")
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
            .child(self.pull_to_sync(&self.mobile_home_scroll));
        for p in pinned {
            let id = p.id.clone();
            let collapsed = self.mobile_collapsed.get(&id).copied().unwrap_or(false);
            let icon = object_icon(Store::global(cx).read(cx), &p.icon, &p.type_key, &active);
            let name = if p.name.is_empty() {
                "Untitled".to_string()
            } else {
                p.name.clone()
            };
            let body = (!collapsed)
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
            column = column.child(
                div()
                    .flex_none()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(10.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .text_color(theme::fg())
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .id(crate::eid("m-wcard", &id))
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .pt(px(12.))
                                    .pr(px(6.))
                                    .pb(px(12.))
                                    .pl(px(16.))
                                    .cursor_pointer()
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
                                    .child(div().flex_1().min_w_0().truncate().child(name))
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Object(open_id.clone()), cx)
                                    }),
                            )
                            .child(
                                m_chev(crate::eid("m-wcard-chev", &id), !collapsed).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.flip_mobile(&flip_id, cx)
                                    }),
                                ),
                            ),
                    )
                    .when_some(body, |d, view| {
                        d.child(div().px(px(10.)).pb(px(10.)).child(view))
                    }),
            );
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
                        let done = o.done == Some(true);
                        let id = o.id.clone();
                        div()
                            .id(crate::eid("m-row-check", &o.id))
                            .flex()
                            .flex_none()
                            .mr(px(6.))
                            .text_color(if done {
                                theme::accent()
                            } else {
                                theme::muted()
                            })
                            .child(checkbox_icon(done, 17.))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let task = roostr_core::api::note::set_field(
                                    &id,
                                    "done",
                                    Value::bool(!done),
                                    cx,
                                );
                                cx.spawn(async move |cx| {
                                    if task.await.is_ok() {
                                        cx.update(|cx| {
                                            Store::global(cx).update(cx, |s, cx| s.refresh_all(cx))
                                        });
                                    }
                                })
                                .detach();
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
                    m_row(crate::eid("m-recent", &o.id))
                        .child(lead)
                        .child(div().min_w_0().truncate().child(name))
                        .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                        .into_any_element()
                })
                .collect()
        };
        column = column.child(self.m_section("__recent", "Recently edited", None, recent_rows, cx));

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
                    m_row(crate::eid("m-type", &t.id))
                        .child(row_icon(&icon))
                        .child(div().min_w_0().truncate().child(name))
                        .when(t.builtin, |d| {
                            d.child(builtin_mark(crate::eid("m-type-lock", &t.id)))
                        })
                        .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                        .into_any_element()
                })
                .collect()
        };
        let add_type = Self::m_section_add("m-new-type")
            .on_click(cx.listener(|this, _, window, cx| this.new_type(window, cx)))
            .into_any_element();
        column = column.child(self.m_section("__types", "Types", Some(add_type), type_rows, cx));

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
                    m_row(crate::eid("m-prop", &r.id))
                        .child(row_icon(&icon))
                        .child(div().min_w_0().truncate().child(name))
                        .when(store.builtin_ids.contains(&r.id), |d| {
                            d.child(builtin_mark(crate::eid("m-prop-lock", &r.id)))
                        })
                        .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx))
                        .into_any_element()
                })
                .collect()
        };
        let add_prop = Self::m_section_add("m-new-prop")
            .child(self.anchor_probe("new-prop"))
            .on_click(cx.listener(|this, _, window, cx| this.open_new_property(window, cx)))
            .into_any_element();
        column =
            column.child(self.m_section("__props", "Properties", Some(add_prop), prop_rows, cx));

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
                        m_btn("m-spaces")
                            .child(back_glyph())
                            .map(|d| self.data_tip(d, "tip-m-spaces", "Spaces", cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mobile_space_open = false;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        m_btn("m-space-settings")
                            .child(
                                svg()
                                    .path(Glyph::Settings.path())
                                    .size(px(18.))
                                    .text_color(theme::fg()),
                            )
                            .map(|d| self.data_tip(d, "tip-m-space-settings", "Space settings", cx))
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
                        div()
                            .id("m-search")
                            .debug_selector(|| "m-search".into())
                            .flex_1()
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded_full()
                            .text_color(theme::muted())
                            .text_size(px(15.))
                            .px(px(16.))
                            .py(px(12.))
                            .cursor_pointer()
                            .child("⌕ Search")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_search(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("m-compose")
                            .debug_selector(|| "m-compose".into())
                            .relative()
                            .flex_none()
                            .size(px(46.))
                            .rounded_full()
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::fg())
                            .text_size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child("＋")
                            .map(|d| self.data_tip(d, "tip-m-compose", "New object", cx))
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
    pub(crate) fn render_m_create_menu(&self, window: &Window) -> Option<AnyElement> {
        let menu = self.create_menu.clone()?;
        let max_h = roostr_core::layout::viewport(window).height - px(96.);
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
                .child(menu)
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
