//! The tab strip (`+layout.svelte:1237-1287`, CSS 2432-2554): one tab per
//! open route of the active space, live titles from the store, drag reorder,
//! middle-click close, separators hidden beside the active tab.

use gpui_kit::component::IconNamed as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{Navigator, Route};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::store::Store;
use roostr_core::{Glyph, theme};

use crate::rail::DragGhost;
use crate::workspace::Workspace;

#[derive(Clone)]
pub(crate) struct TabDrag {
    uid: u64,
    icon: String,
    name: String,
}

/// What a tab shows for its route (`tabInfo`).
struct TabInfo {
    icon: String,
    name: String,
    /// Task layout: a display-only checkbox, `Some(done)`.
    task: Option<bool>,
}

fn tab_info(route: &Route, cx: &App) -> TabInfo {
    let store = Store::global(cx).read(cx);
    let active = Navigator::global(cx).read(cx).space().to_string();
    let Route::Object(id) = route else {
        let name = store
            .space(&active)
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty());
        return TabInfo {
            icon: "🏠".into(),
            name: name.unwrap_or_else(|| "Home".into()),
            task: None,
        };
    };
    let or_untitled = |s: &str| {
        if s.is_empty() {
            "Untitled".to_string()
        } else {
            s.to_string()
        }
    };
    if let Some(o) = store.summary(id) {
        if store.layout_of(&o.type_key) == "task" {
            return TabInfo {
                icon: String::new(),
                name: or_untitled(&o.name),
                task: Some(o.done == Some(true)),
            };
        }
        return TabInfo {
            icon: object_icon(store, &o.icon, &o.type_key, &active),
            name: or_untitled(&o.name),
            task: None,
        };
    }
    if let Some(t) = store.types.iter().find(|t| t.id == *id) {
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
        return TabInfo {
            icon,
            name,
            task: None,
        };
    }
    if let Some(c) = store.space(id) {
        let name = if c.name.is_empty() {
            "Space".to_string()
        } else {
            c.name.clone()
        };
        return TabInfo {
            icon: "🏰".into(),
            name,
            task: None,
        };
    }
    TabInfo {
        icon: "📄".into(),
        name: "Untitled".into(),
        task: None,
    }
}

/// Display-only Anytype checkbox (CheckboxIcon), accent when checked.
pub(crate) fn checkbox_icon(checked: bool, size: f32) -> Svg {
    svg()
        .path(if checked {
            Glyph::CheckboxChecked.path()
        } else {
            Glyph::CheckboxUnchecked.path()
        })
        .size(px(size))
        .flex_none()
        .text_color(if checked {
            theme::accent()
        } else {
            theme::muted()
        })
}

impl Workspace {
    pub(crate) fn render_tab_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = Navigator::global(cx).read(cx);
        let tabs: Vec<(u64, Route)> = nav
            .tabs()
            .iter()
            .map(|t| (t.uid, t.route.clone()))
            .collect();
        let active = nav.active_index();
        let dragging = cx.has_active_drag();
        let drag_uid = self.tab_drag.filter(|_| dragging);
        let over = self.tab_over.filter(|_| dragging);
        let count = tabs.len();
        let uids: Vec<u64> = tabs.iter().map(|t| t.0).collect();
        self.tab_focus.retain(|u, _| uids.contains(u));
        let mut strip = div()
            .id("tab-strip")
            .line_height(relative(1.2))
            .flex_none()
            .h(px(40.))
            .px(px(8.))
            .py(px(5.))
            .flex()
            .items_center()
            .overflow_x_scroll()
            .bg(theme::panel());
        for (i, (uid, route)) in tabs.into_iter().enumerate() {
            let info = tab_info(&route, cx);
            let is_active = i == active;
            let group: SharedString = format!("tab-{uid}").into();
            let over_here = over.filter(|(o, _)| *o == uid).map(|(_, after)| after);
            let payload = TabDrag {
                uid,
                icon: info.icon.clone(),
                name: info.name.clone(),
            };
            let focus = self
                .tab_focus
                .entry(uid)
                .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
                .clone();
            let tab = div()
                .id(("tab", uid))
                .track_focus(&focus)
                .focus_visible(|s| s.border_color(theme::accent()))
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "enter" {
                        Navigator::global(cx).update(cx, |nav, cx| nav.activate(i, cx));
                    }
                }))
                .group(group.clone())
                .relative()
                .flex()
                .items_center()
                .gap(px(7.))
                .min_w_0()
                .max_w(px(220.))
                .flex_shrink(1.)
                .h(px(30.))
                .px(px(9.))
                .rounded(theme::radius::tab())
                .text_size(px(13.))
                .whitespace_nowrap()
                .cursor_pointer()
                .map(|d| {
                    if is_active {
                        d.bg(theme::hl_med()).text_color(theme::fg())
                    } else {
                        d.text_color(theme::muted())
                            .hover(|s| s.bg(theme::hl_light()))
                    }
                })
                .when(drag_uid == Some(uid), |d| d.opacity(0.4))
                .map(|d| match info.task {
                    Some(done) => d.child(checkbox_icon(done, 14.)),
                    None => d.child(
                        div()
                            .flex_none()
                            .text_size(px(13.5))
                            .child(info.icon.clone()),
                    ),
                })
                .child(div().min_w_0().truncate().child(info.name.clone()))
                .child(
                    div()
                        .id(("tab-x", uid))
                        .flex_none()
                        .text_size(px(15.))
                        .line_height(px(15.))
                        .px(px(3.))
                        .py(px(1.))
                        .rounded(theme::radius::xs())
                        .text_color(theme::muted())
                        .opacity(if is_active { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                        .child("×")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            Navigator::global(cx).update(cx, |nav, cx| nav.close(i, cx));
                        })),
                )
                .when_some(over_here, |d, after| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .bottom(px(3.))
                            .w(px(2.))
                            .rounded(px(2.))
                            .bg(theme::accent())
                            .map(|l| {
                                if after {
                                    l.right(px(-3.))
                                } else {
                                    l.left(px(-3.))
                                }
                            }),
                    )
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    Navigator::global(cx).update(cx, |nav, cx| nav.activate(i, cx));
                }))
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(move |_, _, _, cx| {
                        Navigator::global(cx).update(cx, |nav, cx| nav.close(i, cx));
                    }),
                )
                .on_drag(payload, |payload, _, _, cx| {
                    let payload = payload.clone();
                    let ghost = move || {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(theme::radius::tab())
                            .bg(theme::hl_med())
                            .text_size(px(13.))
                            .text_color(theme::fg())
                            .opacity(0.8)
                            .child(payload.icon.clone())
                            .child(payload.name.clone())
                            .into_any_element()
                    };
                    cx.new(|_| DragGhost(Box::new(ghost)))
                })
                .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<TabDrag>, _, cx| {
                    let dragged = e.drag(cx).uid;
                    if this.tab_drag != Some(dragged) {
                        this.tab_drag = Some(dragged);
                        cx.notify();
                    }
                    if dragged == uid {
                        return;
                    }
                    if e.bounds.contains(&e.event.position) {
                        let after = e.event.position.x > e.bounds.center().x;
                        if this.tab_over != Some((uid, after)) {
                            this.tab_over = Some((uid, after));
                            cx.notify();
                        }
                    } else if this.tab_over.is_some_and(|(o, _)| o == uid) {
                        this.tab_over = None;
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(|this, payload: &TabDrag, _, cx| {
                    let target = this.tab_over.take();
                    this.tab_drag = None;
                    cx.notify();
                    let Some((target, after)) = target else {
                        return;
                    };
                    if target == payload.uid {
                        return;
                    }
                    let nav = Navigator::global(cx);
                    let uids: Vec<u64> = nav.read(cx).tabs().iter().map(|t| t.uid).collect();
                    let Some(from) = uids.iter().position(|u| *u == payload.uid) else {
                        return;
                    };
                    let rest: Vec<u64> = uids.into_iter().filter(|u| *u != payload.uid).collect();
                    let Some(ix) = rest.iter().position(|u| *u == target) else {
                        return;
                    };
                    let at = ix + usize::from(after);
                    nav.update(cx, |nav, cx| nav.move_tab(from, at, cx));
                }));
            strip = strip.child(tab);
            if i + 1 < count {
                let hidden = i == active || i + 1 == active;
                strip = strip.child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(16.))
                        .mx(px(2.))
                        .bg(theme::border())
                        .when(hidden, |d| d.opacity(0.)),
                );
            }
        }
        strip.child(
            div()
                .id("tab-new")
                .flex_none()
                .size(px(26.))
                .ml(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::radius::sm())
                .text_color(theme::muted())
                .text_size(px(14.))
                .cursor_pointer()
                .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                .child("＋")
                .map(|d| self.data_tip(d, "tip-new-tab", "New tab", cx))
                .on_click(|_, _, cx| roostr_core::nav::open_tab(Route::Home, false, cx)),
        )
    }
}
