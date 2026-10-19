//! The space rail (`nav.vault`, `+layout.svelte:1317-1374`, CSS 1959-2134):
//! spaces in this person's order with drag reorder, New space, the sync dot,
//! the account/Settings button and the resize handle.

use std::time::Duration;

use gpui_kit::component::IconNamed as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::store::Store;
use roostr_core::sync_status::{sync_progress, sync_tip};
use roostr_core::types::{Space, SyncPhase};
use roostr_core::{Glyph, Navigator, theme};

use crate::modal::Prompt;
use crate::workspace::{RailDrag, Workspace};

/// Drag payload for reordering spaces.
#[derive(Clone)]
pub(crate) struct SpaceDrag {
    id: String,
    icon: String,
    name: String,
}

/// The floating copy of a dragged item (HTML5 drag image). Elements live in a
/// per-frame arena, so it keeps a builder, never a built element.
pub(crate) struct DragGhost(pub Box<dyn Fn() -> AnyElement>);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        (self.0)()
    }
}

/// `color-mix(in srgb, a p%, b)`.
pub(crate) fn mix(a: Hsla, p: f32, b: Hsla) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Rgba {
        r: a.r * p + b.r * (1. - p),
        g: a.g * p + b.g * (1. - p),
        b: a.b * p + b.b * (1. - p),
        a: 1.,
    }
    .into()
}

fn space_icon(space: &Space, size: Pixels, font: Pixels) -> AnyElement {
    let icon = if space.icon.is_empty() {
        space
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "?".into())
    } else {
        space.icon.clone()
    };
    div()
        .flex_none()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .bg(theme::panel())
        .text_color(theme::fg())
        .text_size(font)
        .overflow_hidden()
        .child(roostr_core::ui::icon_text(&icon, size))
        .into_any_element()
}

impl Workspace {
    pub(crate) fn render_rail(&mut self, width: f32, cx: &mut Context<Self>) -> Div {
        let wide = self.rail_width > 100.;
        let store = Store::global(cx).read(cx);
        let spaces: Vec<Space> = store.ordered_spaces().into_iter().cloned().collect();
        let user_color = roostr_core::color::hex(&store.chat_color).unwrap_or_else(|| {
            roostr_core::color::hex(roostr_core::color::DEFAULT_CHAT_COLOR).expect("valid")
        });
        let status = store.status.clone();
        let current = Self::current_space(cx).map(|s| s.id);
        let dragging = cx.has_active_drag();
        let drag_id = self.space_drag.clone().filter(|_| dragging);
        let over = self.space_over.clone().filter(|_| dragging);

        let mut rail = div()
            .ml(px(6.))
            .flex_none()
            .w(px(width))
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .gap(px(4.))
            .py(px(10.))
            .rounded(theme::radius::pane())
            .line_height(relative(1.2))
            .bg(theme::bg())
            .map(|d| {
                if wide {
                    d.px(px(8.)).items_stretch()
                } else {
                    d.items_center()
                }
            });

        for space in spaces {
            let id = space.id.clone();
            let active = current.as_deref() == Some(&id);
            let title = if space.members.is_empty() {
                space.name.clone()
            } else {
                format!("{} · {} member(s)", space.name, space.members.len())
            };
            let over_here = over
                .as_ref()
                .filter(|(o, _)| *o == id)
                .map(|(_, after)| *after);
            let payload = SpaceDrag {
                id: id.clone(),
                icon: space.icon.clone(),
                name: space.name.clone(),
            };
            let tile = div()
                .id(crate::eid("space", &id))
                .relative()
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(10.))
                .text_color(theme::muted())
                .text_size(px(15.))
                .cursor_pointer()
                .map(|d| {
                    if wide {
                        d.w_full().h(px(52.)).px(px(8.)).justify_start()
                    } else {
                        d.size(px(44.)).justify_center()
                    }
                })
                .map(|d| {
                    if active {
                        d.text_color(theme::fg())
                            .bg(mix(user_color, 0.16, theme::panel()))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(10.))
                                    .border_2()
                                    .border_color(user_color),
                            )
                    } else {
                        d.hover(|s| s.bg(theme::panel()))
                    }
                })
                .when(drag_id.as_deref() == Some(&id), |d| d.opacity(0.4))
                .child(space_icon(
                    &space,
                    px(if wide { 38. } else { 36. }),
                    px(if wide { 16. } else { 15. }),
                ))
                .when(wide, |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(space.name.clone()),
                    )
                })
                .when_some(over_here, |d, after| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(-2.))
                            .right(px(-2.))
                            .h(px(2.))
                            .rounded(px(2.))
                            .bg(theme::accent())
                            .map(|l| {
                                if after {
                                    l.bottom(px(-4.))
                                } else {
                                    l.top(px(-4.))
                                }
                            }),
                    )
                })
                .tooltip(crate::tip(title))
                .on_click(cx.listener({
                    let id = id.clone();
                    move |_, _, _, cx| {
                        Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&id, cx));
                    }
                }))
                .on_drag(payload, |payload, _, _, cx| {
                    let payload = payload.clone();
                    let ghost = move || {
                        div()
                            .opacity(0.8)
                            .child(space_icon(
                                &Space {
                                    icon: payload.icon.clone(),
                                    name: payload.name.clone(),
                                    ..Space::default()
                                },
                                px(36.),
                                px(15.),
                            ))
                            .into_any_element()
                    };
                    cx.new(|_| DragGhost(Box::new(ghost)))
                })
                .on_drag_move(cx.listener({
                    let id = id.clone();
                    move |this, e: &DragMoveEvent<SpaceDrag>, _, cx| {
                        let dragged = e.drag(cx).id.clone();
                        if this.space_drag.as_deref() != Some(&dragged) {
                            this.space_drag = Some(dragged.clone());
                            cx.notify();
                        }
                        if dragged == id {
                            return;
                        }
                        if e.bounds.contains(&e.event.position) {
                            let after = e.event.position.y > e.bounds.center().y;
                            if this.space_over != Some((id.clone(), after)) {
                                this.space_over = Some((id.clone(), after));
                                cx.notify();
                            }
                        } else if this.space_over.as_ref().is_some_and(|(o, _)| *o == id) {
                            this.space_over = None;
                            cx.notify();
                        }
                    }
                }))
                .on_drop(cx.listener(|this, payload: &SpaceDrag, _, cx| {
                    let target = this.space_over.take();
                    this.space_drag = None;
                    if let Some((target, after)) = target {
                        commit_space_move(&payload.id, &target, after, cx);
                    }
                    cx.notify();
                }));
            rail = rail.child(tile);
        }

        // New space.
        rail = rail.child(
            div()
                .id("new-space")
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(10.))
                .text_color(theme::muted())
                .text_size(px(15.))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                .map(|d| {
                    if wide {
                        d.w_full().h(px(52.)).px(px(8.))
                    } else {
                        d.size(px(44.)).justify_center()
                    }
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(if wide { 38. } else { 36. }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.))
                        .bg(theme::panel())
                        .text_color(theme::fg())
                        .text_size(px(if wide { 16. } else { 15. }))
                        .child("+"),
                )
                .when(wide, |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child("New space"),
                    )
                })
                .tooltip(crate::tip("New space"))
                .on_click(cx.listener(|this, _, window, cx| this.new_space(window, cx))),
        );
        rail = rail.child(div().flex_1());

        // Sync dot.
        let progress = sync_progress(&status);
        if status.phase != SyncPhase::Live || status.pending.unwrap_or(0) > 0 || progress.is_some()
        {
            let err = status.phase == SyncPhase::Error;
            let pct = progress.and_then(|p| p.pct);
            let dot = div()
                .id("sync-dot")
                .flex_none()
                .size(px(8.))
                .mx_auto()
                .mb(px(6.))
                .rounded_full()
                .tooltip(crate::tip(sync_tip(&status)));
            let dot = match (err, pct) {
                (true, _) => dot.bg(theme::red()).into_any_element(),
                (false, Some(pct)) => dot
                    .bg(theme::border())
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                paint_ring(bounds, pct as f32 / 100., window);
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element(),
                (false, None) => dot
                    .bg(theme::accent())
                    .with_animation(
                        "sync-pulse",
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(ease_in_out),
                        |d, t| d.opacity(1. - 0.7 * (1. - (2. * t - 1.).abs())),
                    )
                    .into_any_element(),
            };
            rail = rail.child(dot);
        }

        // Account / Settings.
        let (profile_name, pic, avatar) = {
            let profile = roostr_core::profile::ProfileCache::global(cx).read(cx);
            (
                profile.name().to_string(),
                profile.picture().to_string(),
                profile.avatar(),
            )
        };
        let label = if profile_name.is_empty() {
            "Settings".to_string()
        } else {
            format!("@{}", profile_name.trim_start_matches('@'))
        };
        rail = rail.child(
            div()
                .id("rail-settings")
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(10.))
                .text_color(theme::muted())
                .text_size(px(17.))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                .map(|d| {
                    if wide {
                        d.w_full().h(px(52.)).px(px(8.))
                    } else {
                        d.size(px(44.)).justify_center()
                    }
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(if wide { 38. } else { 36. }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .overflow_hidden()
                        .map(|d| match (&avatar, pic.is_empty()) {
                            (Some(image), _) => d.child(
                                img(ImageSource::Image(image.clone()))
                                    .size_full()
                                    .rounded_full()
                                    .object_fit(ObjectFit::Cover),
                            ),
                            (None, false) => d.child(
                                img(SharedString::from(pic.clone()))
                                    .size_full()
                                    .rounded_full()
                                    .object_fit(ObjectFit::Cover),
                            ),
                            (None, true) => d.child(
                                svg()
                                    .path(Glyph::Person.path())
                                    .size(px(18.))
                                    .text_color(theme::muted()),
                            ),
                        }),
                )
                .when(wide, |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label),
                    )
                })
                .tooltip(crate::tip("Settings"))
                .on_click(cx.listener(|this, _, window, cx| this.settings(window, cx))),
        );
        rail
    }

    /// `.rail-resize`: a 12px hit zone right of the rail with a centred pill.
    pub(crate) fn render_rail_resize(
        &self,
        rail_col: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.rail_drag.is_some();
        div()
            .id("rail-resize")
            .group("rail-resize")
            .absolute()
            .top(px(6.))
            .bottom(px(6.))
            .left(px(rail_col))
            .w(px(12.))
            .cursor_col_resize()
            .child(
                div()
                    .id("rail-handle")
                    .absolute()
                    .left_0()
                    .w_full()
                    .top(relative(0.5))
                    .map(|d| {
                        if active {
                            d.h(px(64.)).mt(px(-32.))
                        } else {
                            d.h(px(32.)).mt(px(-16.))
                        }
                    })
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(6.))
                            .h_full()
                            .rounded(px(3.))
                            .bg(theme::hover())
                            .opacity(if active { 1. } else { 0. })
                            .group_hover("rail-resize", |s| s.opacity(1.))
                            .hover(|s| s.bg(theme::resize_active())),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.rail_drag = Some(RailDrag::new(e.position.x, this.rail_width));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// `newSpace`: name prompt → `space.create` → refresh → select it.
    pub(crate) fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.new(|cx| Prompt::ask("Space name:", window, cx));
        cx.subscribe(&prompt, |this, _, event: &crate::modal::PromptEvent, cx| {
            this.prompt = None;
            cx.notify();
            let crate::modal::PromptEvent::Submit(name) = event else {
                return;
            };
            if name.is_empty() {
                return;
            }
            let task = roostr_core::api::space::create(name, None, cx);
            cx.spawn(async move |_, cx| {
                let Ok(id) = task.await else { return };
                cx.update(|cx| {
                    Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
                    Navigator::global(cx).update(cx, |nav, cx| nav.use_space(&id, cx));
                });
            })
            .detach();
        })
        .detach();
        self.prompt = Some(prompt);
        cx.notify();
    }
}

/// The conic progress ring (`conic-gradient(accent pct, border 0)`) with a 2px centre dot.
fn paint_ring(bounds: Bounds<Pixels>, fraction: f32, window: &mut Window) {
    let center = bounds.center();
    let r = bounds.size.width / 2.;
    if fraction > 0. {
        let mut path = PathBuilder::fill();
        path.move_to(center);
        let steps = 32;
        for i in 0..=steps {
            let angle = std::f32::consts::TAU * fraction * i as f32 / steps as f32
                - std::f32::consts::FRAC_PI_2;
            path.line_to(point(
                center.x + r * angle.cos(),
                center.y + r * angle.sin(),
            ));
        }
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, theme::accent());
        }
    }
    window.paint_quad(
        fill(
            Bounds::centered_at(center, size(px(4.), px(4.))),
            theme::accent(),
        )
        .corner_radii(px(2.)),
    );
}

/// `commitSpaceMove`: midpoint between the drop neighbours, or a step past the edge.
pub(crate) fn commit_space_move(dragged: &str, target: &str, after: bool, cx: &mut App) {
    if dragged.is_empty() || target.is_empty() || dragged == target {
        return;
    }
    let store = Store::global(cx);
    let position = {
        let store = store.read(cx);
        let order_of = |s: &Space| {
            store
                .space_order
                .get(&s.id)
                .copied()
                .unwrap_or(s.created_at as f64)
        };
        let rest: Vec<&Space> = store
            .ordered_spaces()
            .into_iter()
            .filter(|s| s.id != dragged)
            .collect();
        let Some(ix) = rest.iter().position(|s| s.id == target) else {
            return;
        };
        let at = ix + usize::from(after);
        let before = at.checked_sub(1).and_then(|i| rest.get(i));
        let next = rest.get(at);
        match (before, next) {
            (Some(b), Some(n)) => (order_of(b) + order_of(n)) / 2.,
            (None, Some(n)) => order_of(n) - 1000.,
            (Some(b), None) => order_of(b) + 1000.,
            (None, None) => return,
        }
    };
    let order = store.update(cx, |store, cx| {
        store.space_order.insert(dragged.to_string(), position);
        cx.notify();
        store.space_order.clone()
    });
    roostr_core::api::user_prefs::set_space_order(&order, cx).detach_and_log_err(cx);
}
