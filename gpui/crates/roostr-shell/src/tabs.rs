//! The tab strip (`+layout.svelte:1237-1287`, CSS 2432-2554): one tab per
//! open route of the active space, live titles from the store, drag reorder,
//! middle-click close, separators hidden beside the active tab.

use roostr_core::State;
use roostr_core::nav::Route;
use roostr_core::objects::{object_icon, type_glyph};
use std::collections::HashMap;

use roostr_ui::{Tab, TabBar, Theme, checkbox_display, radius, shadow, ty};
use rune::{Anim, Cursor, Cx, Drag, El, Phase, PointerButton, Spring, Tx, el, row, text};

use crate::shell::or_untitled;

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

/// Points the pointer travels before a press on a tab becomes a drag.
const SLOP: f32 = 4.0;
/// The spring the tabs a drag passes step aside on.
const STEP_ASIDE: Spring = Spring::new(0.22, 0.0);
/// The spring tabs move on otherwise (a drop, a tab opening or closing): Rune's reflow.
const REFLOW: Spring = Spring::new(0.4, 0.0);

/// The tab strip's drag, browser style: the dragged tab follows the pointer along the strip
/// (held inside it), the tabs it passes step aside, and letting go moves it there.
#[derive(Default)]
pub(crate) struct TabDrag {
    /// Each tab's laid-out window rect, by tab uid (offsets aside).
    rects: HashMap<u64, [f32; 4]>,
    /// The tab being dragged, by its index before the drag.
    from: Option<usize>,
    /// The place it would land if let go now.
    to: usize,
    /// Pointer travel along the strip since the press, held so the tab stays in the strip.
    delta: f32,
    /// How far the tabs it passes step aside: its width plus the gap between tabs.
    pitch: f32,
    /// The press that just ended dragged: its release doesn't switch tabs.
    swallow: bool,
}

impl TabDrag {
    /// How far tab `index` is shifted along the strip while a drag runs.
    fn shift(&self, index: usize) -> f32 {
        let Some(from) = self.from else { return 0. };
        if index == from {
            self.delta
        } else if from < index && index <= self.to {
            -self.pitch
        } else if self.to <= index && index < from {
            self.pitch
        } else {
            0.
        }
    }

    /// The pointer is `dx` along the strip from the press on tab `index` of `uids`.
    fn moved(&mut self, uids: &[u64], index: usize, dx: f32) {
        let rect = |i: usize| self.rects.get(&uids[i]).copied();
        let (Some(own), Some(first), Some(last)) = (rect(index), rect(0), rect(uids.len() - 1))
        else {
            return;
        };
        if self.from.is_none() {
            if dx.abs() < SLOP {
                return;
            }
            let gap = match (rect(0), rect(1)) {
                (Some(a), Some(b)) => (b[0] - (a[0] + a[2])).max(0.),
                _ => 0.,
            };
            self.from = Some(index);
            self.pitch = own[2] + gap;
        }
        self.delta = dx.clamp(first[0] - own[0], last[0] + last[2] - (own[0] + own[2]));
        // It lands after every other tab whose middle its own middle has reached.
        let middle = own[0] + own[2] / 2. + self.delta;
        self.to = (0..uids.len())
            .filter(|i| *i != index)
            .filter_map(|i| rect(i))
            .filter(|r| r[0] + r[2] / 2. <= middle)
            .count();
    }
}

/// The drag on tab `index` (uid `uid`): lifts past the slop, follows, and moves it on release.
fn on_tab_drag(s: &mut State, index: usize, d: &Drag) {
    let uids: Vec<u64> = s.nav.tabs().iter().map(|t| t.uid).collect();
    if index >= uids.len() {
        return;
    }
    match d.phase {
        Phase::Down => {
            let drag = s.slot::<TabDrag>();
            drag.from = None;
            drag.swallow = false;
        }
        Phase::Move => s.slot::<TabDrag>().moved(&uids, index, d.dx),
        Phase::Up => {
            let drag = s.slot::<TabDrag>();
            let Some(from) = drag.from.take() else { return };
            drag.swallow = true;
            let to = drag.to;
            drag.delta = 0.;
            if !d.cancelled() && to != from {
                s.nav.move_tab(from, to, &mut s.ui);
            }
        }
    }
}

/// The press that ended a tab drag: true once, and it doesn't switch tabs.
fn take_swallowed(s: &mut State) -> bool {
    std::mem::take(&mut s.slot::<TabDrag>().swallow)
}

/// gpui-kit's TabBar (default Tab variant: the active tab on the kit's
/// active tab fill, labels ellipsised at 220 pt), with Roostr's drag
/// reorder, middle-click and × close and separators on each tab. More tabs
/// than fit scroll sideways (wheel, trackpad, Shift+wheel).
pub(crate) fn view(s: &State, _cx: &mut Cx<State>, t: &Theme) -> El {
    let tabs: Vec<(u64, Route)> = s
        .nav
        .tabs()
        .iter()
        .map(|t| (t.uid, t.route.clone()))
        .collect();
    let active = s.nav.active_index();
    let count = tabs.len();
    let drag = s.get::<TabDrag>();
    let lifted = drag.and_then(|d| d.from);
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
                if take_swallowed(s) {
                    return;
                }
                if tx.button() == PointerButton::Middle {
                    s.nav.close(i, &mut s.ui);
                } else {
                    s.nav.activate(i, &mut s.ui);
                }
            });
        content =
            content.on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| on_tab_drag(s, i, d));
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
        let shift = drag.map_or(0., |d| d.shift(i));
        let lift = lifted == Some(i);
        let reordering = lifted.is_some();
        let (fill, edge) = (t.panel, t.border);
        let tab = Tab::new()
            .key(format!("tab-slot:{uid}"))
            .child(content)
            .suffix(close)
            .wrap(move |e: El| {
                let e = e
                    .on_layout(move |s: &mut State, r: [f32; 4]| {
                        s.slot::<TabDrag>().rects.insert(uid, r);
                    })
                    // Tabs step aside for a drag; a drop, an open or a close reflows them.
                    .motion(if reordering { STEP_ASIDE } else { REFLOW })
                    .offset(shift, 0.);
                if lift {
                    // It follows the pointer exactly, over its neighbours.
                    e.raise()
                        .offset_motion(Anim::INSTANT)
                        .fill(fill)
                        .border(1., edge)
                        .box_shadow(shadow::gpui(4., 12., 0., 0.18))
                } else {
                    e
                }
            });
        bar = bar.child(tab);
    }
    bar.build(t)
}
