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
}

/// Every reorderable list's drag, by list key.
#[derive(Default)]
pub(crate) struct Drags(HashMap<&'static str, DragList>);

/// List `list`'s drag, if one runs.
pub(crate) fn list<'a>(s: &'a State, list: &str) -> Option<&'a DragList> {
    s.get::<Drags>()?
        .0
        .get(list)
        .filter(|l| l.dragging.is_some())
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

/// Make `el` (item `id` of list `list`) draggable to reorder: `drop(s,
/// dragged, target, after)` runs when it is let go over another item.
pub(crate) fn item(
    el: El,
    list: &'static str,
    id: &str,
    axis: Axis,
    drop: fn(&mut State, &str, &str, bool),
) -> El {
    let (rect_id, drag_id) = (id.to_string(), id.to_string());
    el.on_layout(move |s: &mut State, r: [f32; 4]| {
        s.slot::<Drags>()
            .0
            .entry(list)
            .or_default()
            .rects
            .insert(rect_id.clone(), r);
    })
    .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
        let drags = s.slot::<Drags>();
        let l = drags.0.entry(list).or_default();
        match d.phase {
            Phase::Down => {}
            Phase::Move => {
                if l.dragging.is_none() {
                    if d.dx.abs().max(d.dy.abs()) < SLOP {
                        return;
                    }
                    l.dragging = Some(drag_id.clone());
                }
                let p = d.window_point();
                l.point = p;
                l.over = l
                    .rects
                    .iter()
                    .filter(|(other, _)| **other != drag_id)
                    .find(|(_, r)| {
                        p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
                    })
                    .map(|(other, r)| {
                        let after = match axis {
                            Axis::X => p[0] > r[0] + r[2] / 2.,
                            Axis::Y => p[1] > r[1] + r[3] / 2.,
                        };
                        (other.clone(), after)
                    });
            }
            Phase::Up => {
                let dragged = l.dragging.take();
                let over = l.over.take();
                if d.cancelled() {
                    return;
                }
                if let (Some(dragged), Some((target, after))) = (dragged, over) {
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
