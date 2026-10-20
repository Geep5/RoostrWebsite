//! Motion of the sidebar and space-home lists, which the Svelte app popped:
//! a pinned widget or section folding (its body's height and opacity, with
//! the arrow), and pinned widgets gliding to a new place or fading in. Only
//! this device's own toggles, drops and pins move; the first render and
//! whatever sync brings in snap. Also the phone's sync dot cross-fading
//! between states ([`blend`]).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::motion::{self, LayoutGroup, MotionValue, Tween, ValueState};
use web_time::Instant;

/// Pinned widgets gliding to a new place.
const GLIDE: Tween = motion::CONTROL;

/// The time since `epoch` (set on first use) on the executor's clock, the
/// one gpui-motion samples.
fn clock(epoch: &mut Option<Instant>, cx: &App) -> Duration {
    let now = cx.background_executor().now();
    now.saturating_duration_since(*epoch.get_or_insert(now))
}

/// A colour cross-fading from the last one shown.
struct Blend {
    from: Hsla,
    to: Hsla,
    t: MotionValue,
    epoch: Option<Instant>,
}

/// `color` for the element keyed `id`, cross-fading over `tween` from the
/// colour shown when it changes (the first render shows it as is).
pub(crate) fn blend(
    id: impl Into<ElementId>,
    color: Hsla,
    tween: Tween,
    window: &mut Window,
    cx: &mut App,
) -> Hsla {
    let state = motion::use_motion(id, window, cx, || Blend {
        from: color,
        to: color,
        t: MotionValue::new(1.),
        epoch: None,
    });
    state.update(cx, |blend, cx| {
        let now = clock(&mut blend.epoch, cx);
        if blend.to != color {
            let shown = blend.t.sample(now, false).value() as f32;
            blend.from = motion::mix(blend.from, blend.to, shown);
            blend.to = color;
            blend.t.jump(0., now);
            blend.t.animate(1., tween.transition(), now);
        }
        let frame = blend.t.sample(now, motion::reduced(cx));
        if frame.is_animating() {
            window.request_animation_frame();
        }
        motion::mix(blend.from, blend.to, frame.value() as f32)
    })
}

/// A fold's motion: `turn` 0 (folded) ⇄ 1 (open), which the arrow and the
/// body's height ride, and the body's opacity over [`motion::FADE`].
pub(crate) struct Fold {
    open: bool,
    turn: MotionValue,
    fade: MotionValue,
    /// The body's natural height, measured each frame it shows.
    height: Rc<Cell<f32>>,
    epoch: Option<Instant>,
}

/// A [`Fold`] sampled at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FoldStep {
    turn: f32,
    opacity: f32,
    animating: bool,
}

impl FoldStep {
    /// Whether the body renders: open, or still folding away.
    fn shown(self) -> bool {
        self.turn > 0. || self.opacity > 0.
    }
}

impl Fold {
    fn new(open: bool) -> Self {
        let at = if open { 1. } else { 0. };
        Self {
            open,
            turn: MotionValue::new(at),
            fade: MotionValue::new(at),
            height: Rc::default(),
            epoch: None,
        }
    }

    /// The fold at `now`: a flip to `open` eases over `tween`, or snaps
    /// without one (a fold sync brought in).
    fn step(&mut self, open: bool, tween: Option<Tween>, now: Duration, reduced: bool) -> FoldStep {
        if open != self.open {
            self.open = open;
            let target = if open { 1. } else { 0. };
            match tween {
                Some(tween) => {
                    self.turn.animate(target, tween.transition(), now);
                    self.fade.animate(target, motion::FADE.transition(), now);
                }
                None => {
                    self.turn.jump(target, now);
                    self.fade.jump(target, now);
                }
            }
        }
        let turn = self.turn.sample(now, reduced);
        let fade = self.fade.sample(now, reduced);
        FoldStep {
            turn: turn.value().clamp(0., 1.) as f32,
            opacity: fade.value().clamp(0., 1.) as f32,
            animating: turn.is_animating() || fade.is_animating(),
        }
    }
}

/// A fold as rendered this frame.
pub(crate) struct FoldFrame {
    step: FoldStep,
    /// The body's clip while it slides; `None` = its natural height.
    clip: Option<f32>,
    height: Rc<Cell<f32>>,
}

impl FoldFrame {
    /// The arrow's turn: 0 folded, 1 open.
    pub(crate) fn turn(&self) -> f32 {
        self.step.turn
    }

    /// Whether the body renders: open, or still folding away.
    pub(crate) fn shown(&self) -> bool {
        self.step.shown()
    }

    /// `body` faded and clipped to the fold, measured for the next fold (a
    /// flex column, so the body's margins count in its height).
    pub(crate) fn wrap(&self, body: impl IntoElement) -> Div {
        let height = self.height.clone();
        div()
            .when_some(self.clip, |d, clip| d.h(px(clip)).overflow_hidden())
            .when(self.step.opacity < 1., |d| d.opacity(self.step.opacity))
            .child(
                div().relative().flex().flex_col().child(body).child(
                    canvas(
                        move |bounds, _, _| height.set(f32::from(bounds.size.height)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
            )
    }
}

/// The fold keyed `id`, open or not: a flip eases over `tween` (snaps
/// without one, as on the first render), the body sliding to its height
/// when `slide` (else it only fades).
pub(crate) fn fold(
    id: impl Into<ElementId>,
    open: bool,
    tween: Option<Tween>,
    slide: bool,
    window: &mut Window,
    cx: &mut App,
) -> FoldFrame {
    let state = motion::use_motion(id, window, cx, || Fold::new(open));
    state.update(cx, |fold, cx| {
        let now = clock(&mut fold.epoch, cx);
        let step = fold.step(open, tween, now, motion::reduced(cx));
        if step.animating {
            window.request_animation_frame();
        }
        FoldFrame {
            step,
            clip: (slide && step.animating).then(|| fold.height.get() * step.turn),
            height: fold.height.clone(),
        }
    })
}

/// Pinned widgets' places: after this device's drop reorders them, or one
/// pin arrives or leaves, the cards glide (position only) to their new
/// places and an arriving one fades in. A space switch, the first render and
/// several pins changing at once (sync) snap.
pub(crate) struct PinMotion {
    space: String,
    order: Vec<String>,
    layout: LayoutGroup,
    /// Eases over [`GLIDE`] after each change that glides; cards glide while
    /// it moves.
    gate: ValueState,
    gate_at: f64,
    /// This device dropped a widget: its reorder glides.
    armed: bool,
    /// The pin fading in.
    entering: Option<String>,
}

impl PinMotion {
    /// The next reorder is this device's own (a widget dropped).
    pub(crate) fn arm(state: &Entity<Self>, cx: &mut App) {
        state.update(cx, |pins, _| pins.armed = true);
    }

    /// Note the space's pin order; whether the change glides.
    fn note_order(&mut self, space: &str, order: &[String]) -> bool {
        if self.space != space {
            self.space = space.to_string();
            self.order = order.to_vec();
            self.armed = false;
            self.entering = None;
            return false;
        }
        if self.order == order {
            return false;
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
        glide
    }
}

/// The pinned cards' motion this frame ([`pins`]).
pub(crate) struct Pins {
    pub(crate) state: Entity<PinMotion>,
    layout: LayoutGroup,
    glide: bool,
    entering: Option<String>,
}

/// The motion of `space`'s pinned cards (in `order`, the space's pin ids),
/// keyed `id`.
pub(crate) fn pins(
    id: impl Into<ElementId>,
    space: &str,
    order: &[String],
    window: &mut Window,
    cx: &mut App,
) -> Pins {
    let state = motion::use_motion(id, window, cx, || PinMotion {
        space: space.to_string(),
        order: order.to_vec(),
        layout: LayoutGroup::default(),
        gate: ValueState::new(0.),
        gate_at: 0.,
        armed: false,
        entering: None,
    });
    let (layout, glide, entering) = state.update(cx, |pins, cx| {
        if pins.note_order(space, order) {
            pins.gate_at = 1. - pins.gate_at;
        }
        let glide = pins
            .gate
            .frame(pins.gate_at, GLIDE.transition(), window, cx)
            .is_animating();
        (pins.layout.clone(), glide, pins.entering.clone())
    });
    Pins {
        state,
        layout,
        glide,
        entering,
    }
}

impl Pins {
    /// Pin `id`'s card, gliding with its siblings, fading in if it arrived.
    pub(crate) fn card(
        &self,
        id: &str,
        card: impl IntoElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut inner = div().child(card);
        if self.entering.as_deref() == Some(id) {
            let frame = motion::appear_frame(crate::eid("pin-enter", id), -4., window, cx);
            if !frame.is_animating() {
                self.state.update(cx, |pins, _| pins.entering = None);
            }
            inner = frame.apply(inner);
        }
        self.follow(&format!("pin:{id}"), div().flex_none().child(inner))
    }

    /// `element` (keyed `key`) in or below the pinned cards, gliding with them.
    pub(crate) fn follow(&self, key: &str, element: impl IntoElement + 'static) -> AnyElement {
        self.layout
            .position(crate::eid("pin-place", key), element)
            .transition(GLIDE.transition())
            .enabled(self.glide)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would shadow `#[test]`.
    use std::time::Duration;

    use roostr_core::motion::{self, LayoutGroup, ValueState};

    use super::{Fold, PinMotion};

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn fold_ends_folded_and_open() {
        let mut fold = Fold::new(true);
        let start = fold.step(false, Some(motion::CONTROL), MS(0), false);
        assert_eq!((start.turn, start.opacity), (1., 1.));
        assert!(start.animating && start.shown());
        let mid = fold.step(false, Some(motion::CONTROL), MS(75), false);
        assert!(mid.turn > 0. && mid.turn < 1. && mid.animating);
        let folded = fold.step(
            false,
            Some(motion::CONTROL),
            motion::CONTROL.duration,
            false,
        );
        assert_eq!((folded.turn, folded.opacity), (0., 0.));
        assert!(!folded.animating && !folded.shown());

        let opening = fold.step(true, Some(motion::CONTROL), MS(200), false);
        assert!(opening.animating && opening.turn < 1.);
        let open = fold.step(true, Some(motion::CONTROL), MS(350), false);
        assert_eq!((open.turn, open.opacity), (1., 1.));
        assert!(!open.animating && open.shown());
    }

    #[test]
    fn fold_snaps_under_reduced_motion_and_from_sync() {
        let mut fold = Fold::new(true);
        let reduced = fold.step(false, Some(motion::CONTROL), MS(0), true);
        assert_eq!((reduced.turn, reduced.opacity), (0., 0.));
        assert!(!reduced.animating && !reduced.shown());

        let synced = fold.step(true, None, MS(10), false);
        assert_eq!((synced.turn, synced.opacity), (1., 1.));
        assert!(!synced.animating);
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn pin_motion(order: &[&str]) -> PinMotion {
        PinMotion {
            space: "s".into(),
            order: ids(order),
            layout: LayoutGroup::default(),
            gate: ValueState::new(0.),
            gate_at: 0.,
            armed: false,
            entering: None,
        }
    }

    #[test]
    fn only_own_reorders_and_single_pins_glide() {
        let mut pins = pin_motion(&["a", "b", "c"]);
        // A reorder from elsewhere snaps; this device's drop glides.
        assert!(!pins.note_order("s", &ids(&["b", "a", "c"])));
        pins.armed = true;
        assert!(pins.note_order("s", &ids(&["a", "b", "c"])));
        assert!(!pins.armed);
        // One pin arriving fades in; one leaving glides the rest up.
        assert!(pins.note_order("s", &ids(&["a", "b", "c", "d"])));
        assert_eq!(pins.entering.as_deref(), Some("d"));
        assert!(pins.note_order("s", &ids(&["a", "c", "d"])));
        // Several at once, or another space, snap.
        assert!(!pins.note_order("s", &ids(&["x", "y"])));
        assert!(!pins.note_order("t", &ids(&["x"])));
    }
}
