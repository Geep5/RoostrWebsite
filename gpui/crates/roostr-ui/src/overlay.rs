//! Shared plumbing for everything that floats above the view: anchoring a surface to its
//! trigger, the outside-press scrim and the modal that closes it, and the handler list
//! builders attach once built.
//!
//! An overlay builder returns its trigger in place and pushes its surface (and scrim) into the
//! view's `layer`, which the view mounts with [`crate::root`]; the surface is `El::anchored` to
//! the trigger's key, so it follows layout and flips when it doesn't fit.

use std::rc::Rc;

use rune::{El, Side, Transition, Tx};

pub use rune_kit::{Placement, scrim};

use crate::motion;

/// `content` placed next to the element keyed `anchor`, appearing as Roostr's panels do
/// ([`motion::appear`]: fade in while sliding `from_y` points into place over `FADE`; no exit
/// animation). Above or below the anchor it is at most as tall as the window leaves it
/// (`El::anchored_fit`): give a long body a scroll viewport that shrinks.
pub fn anchored(content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = split(placement);
    let from_y = match side {
        Side::Top => 4.0,
        Side::Bottom => -4.0,
        Side::Left | Side::Right => 0.0,
    };
    let content = if matches!(side, Side::Top | Side::Bottom) {
        content.anchored_fit(anchor, side, align, gap)
    } else {
        content.anchored(anchor, side, align, gap)
    };
    content.transition(motion::appear(from_y)).instant_changes()
}

/// `content` placed next to `anchor` with no entrance at all (lands on its first frame).
pub fn anchored_still(content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = split(placement);
    content
        .anchored(anchor, side, align, gap)
        .transition(Transition::none())
        .instant_changes()
}

/// The side and alignment a placement names.
pub fn split(p: Placement) -> (Side, rune::Align) {
    use rune::Align;
    match p {
        Placement::Top => (Side::Top, Align::Center),
        Placement::TopStart => (Side::Top, Align::Start),
        Placement::TopEnd => (Side::Top, Align::End),
        Placement::Bottom => (Side::Bottom, Align::Center),
        Placement::BottomStart => (Side::Bottom, Align::Start),
        Placement::BottomEnd => (Side::Bottom, Align::End),
        Placement::Left => (Side::Left, Align::Center),
        Placement::Right => (Side::Right, Align::Center),
        Placement::RightStart => (Side::Right, Align::Start),
    }
}

/// Makes `surface` dismissable: pushes a scrim keyed `{key}-scrim` that closes on an outside
/// press, and makes the surface modal (focus trapped, Escape closes, focus returns to the
/// opener when it goes).
pub fn dismissable<S: 'static>(
    surface: El,
    key: &str,
    close: impl Fn(&mut S) + 'static,
    layer: &mut Vec<El>,
) -> El {
    let close = Rc::new(close);
    let outside = close.clone();
    layer.push(scrim(format!("{key}-scrim"), move |s: &mut S| outside(s)));
    surface.modal(true, move |s: &mut S| close(s))
}

type Apply = Box<dyn FnOnce(El) -> El>;

/// Handlers a builder collects and attaches to its root once built (dropped when disabled).
#[derive(Default)]
pub struct Acts(Vec<Apply>);

impl Acts {
    pub fn press<S: 'static>(&mut self, f: impl Fn(&mut S) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_press(f)));
    }
    pub fn press_tx<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_press_tx(f)));
    }
    /// Any other attachment (`on_secondary_press`, `on_hover`, …).
    pub fn with(&mut self, f: impl FnOnce(El) -> El + 'static) {
        self.0.push(Box::new(f));
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Attaches every handler to `e` when `enabled`.
    pub fn apply(self, mut e: El, enabled: bool) -> El {
        if enabled {
            for f in self.0 {
                e = f(e);
            }
        }
        e
    }
}
