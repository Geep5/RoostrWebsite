//! Motion of the sidebar and space-home lists, which the Svelte app popped:
//! a pinned widget or section folding (its body's height and opacity, with
//! the arrow), and pinned widgets gliding to a new place or fading in. Only
//! this device's own toggles, drops and pins move; the first render and
//! whatever sync brings in snap.
//!
//! Rune animates layout and presence itself: a fold is its body mounting or
//! unmounting in a clipped box whose height springs, the arrow is a rotation
//! on a curve, and the pinned cards are keyed rows whose reorder glides. What
//! must snap is drawn with `instant_changes`.

use std::collections::HashMap;
use std::time::Duration;

use roostr_core::State;
use roostr_ui::{Glyph, glyph, motion};
use rune::{Effect, El, Spring, SpringPair, Transition, col};
use web_time::Instant;

/// Pinned widgets gliding to a new place.
const GLIDE: motion::Tween = motion::CONTROL;
/// How long a commit that must snap (a change sync brought in) is drawn instantly.
const SNAP_WINDOW: Duration = Duration::from_millis(50);

/// The ▶ disclosure arrow, turned 90° while `open`, turning over [`motion::CONTROL`].
pub(crate) fn arrow(open: bool, size: f32, color: rune::Color) -> El {
    glyph(Glyph::Disclosure, size)
        .color(color)
        .rotate(if open {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        })
        .rotate_motion(motion::CONTROL.anim())
}

/// A fold's body box keyed `key`: `body` (present while open) fades in and out
/// over [`motion::FADE`] while the box's height slides over [`motion::CONTROL`]
/// (`slide`; a long list only fades). `animate` false (a fold sync brought in,
/// the first render) snaps.
pub(crate) fn fold(key: impl Into<String>, animate: bool, slide: bool, body: Option<El>) -> El {
    let fade = Transition {
        insertion: Effect::opacity(0.0),
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(motion::FADE.spring(), motion::FADE.spring()),
        ..Transition::identity()
    };
    let mut boxed = col().key(key).w_full().clip_rounded(0.0).motion(if slide {
        motion::CONTROL.spring()
    } else {
        Spring::NONE
    });
    if let Some(body) = body {
        boxed = boxed.child(body.transition(fade));
    }
    if animate {
        boxed
    } else {
        boxed.instant_changes()
    }
}

/// One list of pinned cards: its order, and how its last change moves.
#[derive(Default)]
struct PinList {
    space: String,
    order: Vec<String>,
    /// This device dropped a widget: its reorder glides.
    armed: bool,
    /// The pin fading in.
    entering: Option<String>,
    /// The last order change came from elsewhere: commits until then snap.
    snap_until: Option<Instant>,
}

impl PinList {
    /// Note the space's pin order; whether the change glides.
    fn note_order(&mut self, space: &str, order: &[String]) -> Option<bool> {
        if self.space != space {
            self.space = space.to_string();
            self.order = order.to_vec();
            self.armed = false;
            self.entering = None;
            return Some(false);
        }
        if self.order == order {
            return None;
        }
        let added: Vec<&String> = order.iter().filter(|id| !self.order.contains(id)).collect();
        let removed = self.order.iter().filter(|id| !order.contains(id)).count();
        // The next change after a drop is its reorder.
        let armed = std::mem::take(&mut self.armed);
        let glide = match (added.len(), removed) {
            (0, 0) => armed,
            (1, 0) if !self.order.is_empty() => {
                self.entering = Some(added[0].clone());
                true
            }
            (0, 1) => true,
            _ => false,
        };
        self.order = order.to_vec();
        Some(glide)
    }
}

/// Every pinned-card list's motion, by list key (`pins`, `m-pins`).
#[derive(Default)]
pub(crate) struct PinLists(HashMap<&'static str, PinList>);

/// Note list `list`'s current order (from a listener, before the frame that
/// draws it): a change from this device glides, one sync brought in snaps.
/// True when the order changed.
pub(crate) fn note_pins(s: &mut State, list: &'static str, space: &str, order: &[String]) -> bool {
    let now = s.rt.now();
    let pins = s.slot::<PinLists>().0.entry(list).or_default();
    match pins.note_order(space, order) {
        None => false,
        Some(glide) => {
            pins.snap_until = (!glide).then(|| now + SNAP_WINDOW);
            true
        }
    }
}

/// The next reorder of list `list` is this device's own (a widget dropped).
pub(crate) fn arm(s: &mut State, list: &'static str) {
    s.slot::<PinLists>().0.entry(list).or_default().armed = true;
}

/// List `list`'s column of cards, snapping while a change sync brought in lands.
/// Its children glide to new places over [`GLIDE`].
pub(crate) fn pin_list(s: &State, list: &'static str, column: El) -> El {
    let snap = s
        .get::<PinLists>()
        .and_then(|l| l.0.get(list))
        .and_then(|p| p.snap_until)
        .is_some_and(|until| s.rt.now() < until);
    if snap {
        column.instant_changes()
    } else {
        column
    }
}

/// Pin `id`'s card in list `list`: it glides with its siblings and fades in
/// (rising 4 pt) when it is the pin that just arrived.
pub(crate) fn pin_card(s: &State, list: &'static str, id: &str, card: El) -> El {
    let entering = s
        .get::<PinLists>()
        .and_then(|l| l.0.get(list))
        .is_some_and(|p| p.entering.as_deref() == Some(id));
    let card = card.motion(GLIDE.spring());
    if entering {
        card.transition(motion::appear(-4.0))
    } else {
        card
    }
}

#[cfg(test)]
mod tests {
    use super::PinList;

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn only_own_reorders_and_single_pins_glide() {
        let mut pins = PinList {
            space: "s".into(),
            order: ids(&["a", "b", "c"]),
            ..PinList::default()
        };
        // A reorder from elsewhere snaps; this device's drop glides.
        assert_eq!(pins.note_order("s", &ids(&["b", "a", "c"])), Some(false));
        pins.armed = true;
        assert_eq!(pins.note_order("s", &ids(&["a", "b", "c"])), Some(true));
        assert!(!pins.armed);
        assert_eq!(pins.note_order("s", &ids(&["a", "b", "c"])), None);
        // One pin arriving fades in; one leaving glides the rest up.
        assert_eq!(
            pins.note_order("s", &ids(&["a", "b", "c", "d"])),
            Some(true)
        );
        assert_eq!(pins.entering.as_deref(), Some("d"));
        assert_eq!(pins.note_order("s", &ids(&["a", "c", "d"])), Some(true));
        // Several at once, or another space, snap.
        assert_eq!(pins.note_order("s", &ids(&["x", "y"])), Some(false));
        assert_eq!(pins.note_order("t", &ids(&["x"])), Some(false));
    }
}
