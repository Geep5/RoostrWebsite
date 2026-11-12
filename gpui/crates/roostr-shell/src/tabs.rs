//! The tab strip (`+layout.svelte:1237-1287`, CSS 2432-2554): one tab per
//! open route of the active space, live titles from the store, drag reorder,
//! middle-click close, separators hidden beside the active tab.

use roostr_core::State;
use roostr_core::nav::Route;
use roostr_core::objects::{object_icon, type_glyph};
use roostr_ui::{Glyph, Tab, TabBar, Theme, checkbox_display, radius, ty};
use rune::{Cursor, Cx, El, PointerButton, Tx, el, row, text};

use crate::drag::{self, Axis};
use crate::shell::{fill, fill_glyph, or_untitled};

/// What a tab shows for its route (`tabInfo`).
pub(crate) struct TabInfo {
    pub(crate) icon: String,
    pub(crate) name: String,
    /// Task layout: a display-only checkbox, `Some(done)`.
    pub(crate) task: Option<bool>,
}

pub(crate) fn tab_info(s: &State, route: &Route) -> TabInfo {
    let store = &s.store;
    let active = s.nav.space();
    let Route::Object(id) = route else {
        let name = store
            .space(active)
            .map(|sp| sp.name.clone())
            .filter(|n| !n.is_empty());
        return TabInfo {
            icon: "🏠".into(),
            name: name.unwrap_or_else(|| "Home".into()),
            task: None,
        };
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
            icon: object_icon(store, &o.icon, &o.type_key, active),
            name: or_untitled(&o.name),
            task: None,
        };
    }
    if let Some(ty) = store.types.iter().find(|ty| ty.id == *id) {
        return TabInfo {
            icon: if ty.icon.is_empty() {
                type_glyph(store, &ty.key, active)
            } else {
                ty.icon.clone()
            },
            name: if ty.name.is_empty() {
                ty.key.clone()
            } else {
                ty.name.clone()
            },
            task: None,
        };
    }
    if let Some(c) = store.space(id) {
        return TabInfo {
            icon: "🏰".into(),
            name: if c.name.is_empty() {
                "Space".to_string()
            } else {
                c.name.clone()
            },
            task: None,
        };
    }
    TabInfo {
        icon: "📄".into(),
        name: "Untitled".into(),
        task: None,
    }
}

/// A tab dropped next to another: `to` is its place among the others.
fn drop_tab(s: &mut State, dragged: &str, target: &str, after: bool) {
    let uids: Vec<String> = s.nav.tabs().iter().map(|t| t.uid.to_string()).collect();
    let Some(from) = uids.iter().position(|u| u == dragged) else {
        return;
    };
    let rest: Vec<&String> = uids.iter().filter(|u| *u != dragged).collect();
    let Some(ix) = rest.iter().position(|u| *u == target) else {
        return;
    };
    let at = ix + usize::from(after);
    s.nav.move_tab(from, at, &mut s.ui);
}

/// gpui-kit's TabBar (default Tab variant: the active tab on the kit's
/// active tab fill, labels ellipsised at 220 pt), with Roostr's drag
/// reorder, middle-click and × close and separators on each tab. More tabs
/// than fit scroll sideways (wheel, trackpad, Shift+wheel); New tab stays put.
pub(crate) fn view(s: &State, _cx: &mut Cx<State>, t: &Theme) -> El {
    let tabs: Vec<(u64, Route)> = s
        .nav
        .tabs()
        .iter()
        .map(|t| (t.uid, t.route.clone()))
        .collect();
    let active = s.nav.active_index();
    let count = tabs.len();
    let dragging = drag::dragging(s, "tabs").map(str::to_string);
    let mut bar = TabBar::new("tab-strip")
        .selected_index(active)
        .max_width(220.)
        .h(40.)
        .scrollable()
        .px(8.)
        .fill(t.panel)
        .on_click(|s: &mut State, ix: usize| s.nav.activate(ix, &mut s.ui));
    for (i, (uid, route)) in tabs.into_iter().enumerate() {
        let info = tab_info(s, &route);
        let is_active = i == active;
        let lead = match info.task {
            Some(done) => checkbox_display(format!("tab-check:{uid}"), done)
                .small()
                .build(t),
            None => text(info.icon.clone()).font(ty::ui(13.5)),
        };
        let mut content = row()
            .key(format!("tab:{uid}"))
            .items_center()
            .gap(7.)
            .min_w(0.)
            .label(info.name.clone())
            .child(lead)
            .child(
                text(info.name.clone())
                    .font(ty::ui(13.))
                    .color(if is_active {
                        t.kit.tab_active_foreground
                    } else {
                        t.kit.tab_foreground
                    })
                    .truncate()
                    .shrink()
                    .min_w(0.),
            )
            // Middle-click closes.
            .pointer_buttons([PointerButton::Middle])
            .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                if tx.button() == PointerButton::Middle {
                    s.nav.close(i, &mut s.ui);
                } else {
                    s.nav.activate(i, &mut s.ui);
                }
            });
        if dragging.as_deref() == Some(uid.to_string().as_str()) {
            content = content.opacity(0.4);
        }
        let content = drag::item(content, "tabs", &uid.to_string(), Axis::X, drop_tab);
        // The separator after this tab, hidden beside the active one.
        let mut close = row().items_center().pr(6.).child(
            row()
                .key(format!("tab-x:{uid}"))
                .h(17.)
                .px(3.)
                .py(1.)
                .radius(radius::XS)
                .center()
                .cursor(Cursor::Pointer)
                .hover(t.kit.secondary_hover)
                .group_opacity(if is_active { 1. } else { 0. }, 1.)
                .label("Close tab")
                .child(text("×").font(ty::ui(15.).lh(15.)).color(t.muted))
                .on_press(move |s: &mut State| s.nav.close(i, &mut s.ui)),
        );
        if i + 1 < count {
            let hidden = i == active || i + 1 == active;
            close = close.child(
                el().pin(None, Some(8.), Some(0.), None)
                    .w(1.)
                    .h(16.)
                    .fill(t.border)
                    .opacity(if hidden { 0. } else { 1. }),
            );
        }
        if let Some(after) = drag::over(s, "tabs", &uid.to_string()) {
            let (left, right) = if after {
                (None, Some(0.))
            } else {
                (Some(0.), None)
            };
            close = close.child(
                el().pin(left, Some(3.), right, Some(3.))
                    .w(2.)
                    .radius(2.)
                    .fill(t.accent),
            );
        }
        bar = bar.child(Tab::new().child(content).suffix(close));
    }
    let new_tab = fill("tab-new", t.hl_med)
        .size(26., 26.)
        .ml(4.)
        .radius(radius::SM)
        .label("New tab")
        .child(fill_glyph(t, Glyph::Plus, 14.))
        .on_press(|s: &mut State| roostr_core::nav::open_tab(Route::Home, false, s));
    bar.suffix(roostr_ui::tooltip(new_tab, "New tab")).build(t)
}

/// The floating copy of a dragged tab.
pub(crate) fn tab_ghost(s: &State, t: &Theme, layer: &mut Vec<El>) {
    let Some(uid) = drag::dragging(s, "tabs") else {
        return;
    };
    let Some(tab) = s.nav.tabs().iter().find(|tab| tab.uid.to_string() == uid) else {
        return;
    };
    let info = tab_info(s, &tab.route);
    let ghost = row()
        .items_center()
        .gap(7.)
        .h(30.)
        .px(10.)
        .radius(radius::TAB)
        .fill(t.hl_med)
        .opacity(0.8)
        .child(text(info.icon).font(ty::ui(13.)))
        .child(text(info.name).font(ty::ui(13.)).color(t.fg));
    drag::ghost(s, "tabs", ghost, layer);
}
