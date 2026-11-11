//! Drag-to-reorder for the rail's spaces, the tab strip and the pinned
//! widgets (GPUI's `on_drag` / `on_drag_move` / `on_drop` with a floating
//! ghost): the dragged item dims, a 2 pt accent line marks where it would
//! land (before or after the item under the pointer), a ghost follows the
//! pointer, and the drop commits the move.

use std::collections::HashMap;

use roostr_core::State;
use rune::{Drag, El, Phase, Tx};

/// Points the pointer travels before a press becomes a drag.
const SLOP: f32 = 4.0;

/// Which way a list runs: where "after" is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Axis {
    X,
    Y,
}

/// One list's drag.
#[derive(Default)]
pub(crate) struct DragList {
    /// Each item's window rect, as last laid out.
    rects: HashMap<String, [f32; 4]>,
    /// The item being dragged (past the slop).
    pub(crate) dragging: Option<String>,
    /// The item under the pointer, and whether the drop lands after it.
    pub(crate) over: Option<(String, bool)>,
    /// The pointer, in window points.
    pub(crate) point: [f32; 2],
    /// The press that just ended dragged: its release opens nothing.
    pub(crate) swallow_press: bool,
}

impl DragList {
    /// `id` is lifted and the pointer is at window point `p`: mark the item
    /// under it and the side the drop lands on.
    pub(crate) fn track(&mut self, id: &str, axis: Axis, p: [f32; 2]) {
        self.dragging = Some(id.to_string());
        self.point = p;
        self.over = self
            .rects
            .iter()
            .filter(|(other, _)| *other != id)
            .find(|(_, r)| p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3])
            .map(|(other, r)| {
                let after = match axis {
                    Axis::X => p[0] > r[0] + r[2] / 2.,
                    Axis::Y => p[1] > r[1] + r[3] / 2.,
                };
                (other.clone(), after)
            });
    }

    /// The press let go: the (dragged, target, after) drop, unless `cancelled`
    /// or it was no drag. A drag swallows the press its release would make.
    pub(crate) fn finish(&mut self, cancelled: bool) -> Option<(String, String, bool)> {
        let dragged = self.dragging.take();
        let over = self.over.take();
        self.swallow_press = dragged.is_some();
        match (dragged, over, cancelled) {
            (Some(dragged), Some((target, after)), false) => Some((dragged, target, after)),
            _ => None,
        }
    }
}

/// Every reorderable list's drag, by list key.
#[derive(Default)]
pub(crate) struct Drags(HashMap<String, DragList>);

/// List `list`'s drag state (made on first use).
pub(crate) fn state<'a>(s: &'a mut State, list: &str) -> &'a mut DragList {
    s.slot::<Drags>().0.entry(list.to_string()).or_default()
}

/// List `list`'s drag, if one runs.
pub(crate) fn list<'a>(s: &'a State, list: &str) -> Option<&'a DragList> {
    s.get::<Drags>()?
        .0
        .get(list)
        .filter(|l| l.dragging.is_some())
}

/// Whether any list's item is being dragged: hover effects under the pointer
/// stay off meanwhile (a fold arrow passed over doesn't light up).
pub(crate) fn any(s: &State) -> bool {
    s.get::<Drags>()
        .is_some_and(|d| d.0.values().any(|l| l.dragging.is_some()))
}

/// The item of list `list` being dragged.
pub(crate) fn dragging<'a>(s: &'a State, l: &str) -> Option<&'a str> {
    list(s, l)?.dragging.as_deref()
}

/// Where the drop would land: `Some(after)` when it lands next to `id`.
pub(crate) fn over(s: &State, l: &str, id: &str) -> Option<bool> {
    list(s, l)?
        .over
        .as_ref()
        .filter(|(o, _)| o == id)
        .map(|(_, after)| *after)
}

/// The press on an item of list `l` that just ended was a drag (its release
/// must not press the item): true once, then cleared.
pub(crate) fn take_swallowed_press(s: &mut State, l: &str) -> bool {
    std::mem::take(&mut state(s, l).swallow_press)
}

/// Record item `id` of list `list`'s window rect `r` as laid out.
pub(crate) fn place(s: &mut State, list: &str, id: &str, r: [f32; 4]) {
    state(s, list).rects.insert(id.to_string(), r);
}

/// Make `el` (item `id` of list `list`) draggable to reorder: `drop(s,
/// dragged, target, after)` runs when it is let go over another item.
pub(crate) fn item(
    el: El,
    list: impl Into<String>,
    id: &str,
    axis: Axis,
    drop: impl Fn(&mut State, &str, &str, bool) + 'static,
) -> El {
    let list: String = list.into();
    let layout_list = list.clone();
    let rect_id = id.to_string();
    handle(el, list, id, axis, drop)
        .on_layout(move |s: &mut State, r: [f32; 4]| place(s, &layout_list, &rect_id, r))
}

/// Make `el`, a part of item `id` with handlers of its own (a pinned card's
/// head), start item `id`'s drag too: Rune hands a press to the innermost
/// element with handlers, so the item's own drag never sees it. Its press
/// handler must skip a press [`take_swallowed_press`] reports.
pub(crate) fn handle(
    el: El,
    list: impl Into<String>,
    id: &str,
    axis: Axis,
    drop: impl Fn(&mut State, &str, &str, bool) + 'static,
) -> El {
    let list: String = list.into();
    let drag_id = id.to_string();
    el.on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
        let l = state(s, &list);
        match d.phase {
            Phase::Down => l.swallow_press = false,
            Phase::Move => {
                if l.dragging.is_none() && d.dx.abs().max(d.dy.abs()) < SLOP {
                    return;
                }
                l.track(&drag_id, axis, d.window_point());
            }
            Phase::Up => {
                if let Some((dragged, target, after)) = l.finish(d.cancelled()) {
                    drop(s, &dragged, &target, after);
                }
            }
        }
    })
}

/// The ghost of list `list`'s drag: `ghost` placed at the pointer (offset a
/// little so the pointer stays visible), pushed into the overlay layer.
pub(crate) fn ghost(s: &State, l: &str, ghost: El, layer: &mut Vec<El>) {
    if let Some(drag) = list(s, l) {
        let [x, y] = drag.point;
        layer.push(
            rune::el()
                .absolute(x + 8., y + 8.)
                .child(ghost)
                .inert()
                .transition(rune::Transition::none()),
        );
    }
}
