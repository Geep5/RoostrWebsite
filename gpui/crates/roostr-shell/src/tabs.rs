//! The tab strip (`+layout.svelte:1237-1287`, CSS 2432-2554): one tab per
//! open route of the active space, live titles from the store, drag reorder,
//! middle-click close, separators hidden beside the active tab.

use gpui_kit::component::IconNamed as _;
use gpui_kit::component::tab::{Tab, TabBar};
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
        // gpui-kit's TabBar (default Tab variant: the active tab on
        // `tab.active.background`, labels ellipsised at 220px, a clicked tab
        // scrolled into view). Roostr's drag reorder, middle-click and ×
        // close, keyboard focus and separators ride on each Tab.
        let mut bar = TabBar::new("tab-strip")
            .selected_index(active)
            .max_width(px(220.))
            .flex_none()
            .h(px(40.))
            .px(px(8.))
            .line_height(relative(1.2))
            .bg(theme::panel())
            .on_click(|ix, _, cx| {
                let ix = *ix;
                Navigator::global(cx).update(cx, |nav, cx| nav.activate(ix, cx));
            });
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
            // Icon (or the Anytype checkbox on a task) and the ellipsised name.
            let content = div()
                .flex()
                .items_center()
                .gap(px(7.))
                .min_w_0()
                .map(|d| match info.task {
                    Some(done) => d.child(checkbox_icon(done, 14.)),
                    None => d.child(
                        div()
                            .flex_none()
                            .text_size(px(13.5))
                            .child(info.icon.clone()),
                    ),
                })
                .child(div().min_w_0().truncate().child(info.name.clone()));
            // The separator after this tab, hidden beside the active one.
            let separator = (i + 1 < count).then(|| {
                let hidden = i == active || i + 1 == active;
                div()
                    .absolute()
                    .right_0()
                    .top(px(8.))
                    .w(px(1.))
                    .h(px(16.))
                    .bg(theme::border())
                    .when(hidden, |d| d.opacity(0.))
            });
            let drop_line = over_here.map(|after| {
                div()
                    .absolute()
                    .top(px(3.))
                    .bottom(px(3.))
                    .w(px(2.))
                    .rounded(px(2.))
                    .bg(theme::accent())
                    .map(|l| if after { l.right_0() } else { l.left_0() })
            });
            let close = div()
                .flex()
                .items_center()
                .pr(px(6.))
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
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                        .child("×")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            Navigator::global(cx).update(cx, |nav, cx| nav.close(i, cx));
                        })),
                )
                .children(separator)
                .children(drop_line);
            let tab = Tab::new()
                .aria_label(info.name.clone())
                .child(content)
                .suffix(close)
                .track_focus(&focus)
                .focus_visible(|s| s.border_1().border_color(theme::accent()))
                .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "enter" {
                        Navigator::global(cx).update(cx, |nav, cx| nav.activate(i, cx));
                    }
                }))
                .group(group)
                .when(drag_uid == Some(uid), |d| d.opacity(0.4))
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
            bar = bar.child(tab);
        }
        bar.suffix(
            div()
                .id("tab-new")
                .group("tab-new")
                .flex_none()
                .size(px(26.))
                .ml(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::radius::sm())
                .cursor_pointer()
                .hover(|s| s.bg(theme::hl_med()))
                .child(
                    svg()
                        .path(Glyph::Plus.path())
                        .size(px(14.))
                        .text_color(theme::muted())
                        .group_hover("tab-new", |s| s.text_color(theme::fg())),
                )
                .map(|d| self.data_tip(d, "tip-new-tab", "New tab", cx))
                .on_click(|_, _, cx| roostr_core::nav::open_tab(Route::Home, false, cx)),
        )
    }
}
