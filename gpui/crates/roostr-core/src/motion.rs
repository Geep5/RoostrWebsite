//! Motion: gpui-motion (vendor/gpui-motion-kit) with the Svelte app's CSS
//! timings as presets. Every UI crate animates through this module.
//!
//! - Tweens match the Svelte CSS (`transition: … 0.15s ease`); CSS `ease` is
//!   [`css_ease`], not gpui-motion's default ease-out.
//! - Springs only settle a gesture release ([`release_spring`]), always with
//!   explicit rest tolerances.
//! - Looping indicators (sync pulse, typing dots, spinners) stay on gpui's
//!   `Animation::repeat`.
//! - Reduced motion snaps every transition to its end state: the platform
//!   supplies the OS setting through [`follow_reduce_motion`].
//! - Motion never gates input: menus, dialogs and sheets take focus and keys
//!   at once, and anything interactive leaves without an exit animation.
//!
//! Stateless components keep their motion state in element state
//! ([`use_motion`], [`animate`]), keyed by a stable id (an object, block or
//! message id, never an index).

use std::time::Duration;

use futures::{Stream, StreamExt as _};
use gpui_kit::{App, ElementId, Entity, Hsla, Rgba, Window};

pub use gpui_motion_kit::core::{
    Bezier, Color, ColorProperty, Easing, Motion, MotionValue, Spring, StyleMotion, StyleTarget,
    SvgMotion, SvgTarget, Target, Transition, ValueFrame,
};
pub use gpui_motion_kit::{
    AutoSize, Frame, LayoutGroup, MotionState, Motions, NumberEffect, NumberMotionState, PopLayout,
    SizeAxis, StyleFrame, StyleMotionState, SvgFrame, SvgMotionState, ValueState, color_to_rgba,
};

/// CSS `ease`: cubic-bezier(0.25, 0.1, 0.25, 1).
const EASE: [f64; 4] = [0.25, 0.1, 0.25, 1.0];
/// The rail handle's cubic-bezier(0.22, 1, 0.36, 1).
const EASE_OUT_QUINT: [f64; 4] = [0.22, 1.0, 0.36, 1.0];

/// A duration and cubic-bezier curve: one of the Svelte app's CSS transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    pub duration: Duration,
    curve: [f64; 4],
}

impl Tween {
    /// `ms` milliseconds of CSS `ease`.
    pub const fn ease(ms: u64) -> Self {
        Self::bezier(ms, EASE)
    }

    /// `ms` milliseconds of cubic-bezier(`curve`).
    pub const fn bezier(ms: u64, curve: [f64; 4]) -> Self {
        Self {
            duration: Duration::from_millis(ms),
            curve,
        }
    }

    /// The same curve over `duration`.
    pub const fn with_duration(self, duration: Duration) -> Self {
        Self { duration, ..self }
    }

    pub fn easing(self) -> Easing {
        let [x1, y1, x2, y2] = self.curve;
        Easing::Bezier(Bezier::new(x1, y1, x2, y2).expect("preset curves are valid"))
    }

    pub fn transition(self) -> Transition {
        Transition::Tween(self.duration, self.easing())
    }
}

impl From<Tween> for Transition {
    fn from(tween: Tween) -> Self {
        tween.transition()
    }
}

/// Hover reveal of row handles and actions (`opacity 0.1s ease`).
pub const HOVER_REVEAL: Tween = Tween::ease(100);
/// Fades: tooltips, drawer tabs, chevrons, menus appearing (`0.12s ease`).
pub const FADE: Tween = Tween::ease(120);
/// Controls: switch knob and track, section and sort arrows, hover colours (`0.15s ease`).
pub const CONTROL: Tween = Tween::ease(150);
/// The rail resize handle (`all 0.15s cubic-bezier(0.22, 1, 0.36, 1)`).
pub const HANDLE: Tween = Tween::bezier(150, EASE_OUT_QUINT);
/// Grid columns and the drawer tabs riding them (`0.16s ease`).
pub const COLUMNS: Tween = Tween::ease(160);
/// A bottom sheet settling back after a short drag (`transform 0.18s ease`).
pub const SHEET_SNAP: Tween = Tween::ease(180);
/// The editor's toggle-block disclosure (`transform 0.2s ease`).
pub const DISCLOSURE: Tween = Tween::ease(200);

/// CSS `ease`, the Svelte app's default transition curve.
pub fn css_ease() -> Easing {
    COLUMNS.easing()
}

/// Settling after a gesture lets go (a sheet flung open or shut): critically
/// damped, reaching 90% in `duration`, at rest within half a pixel.
pub fn release_spring(duration: Duration) -> Transition {
    let spring = Spring::visual(duration, 0.0)
        .and_then(|spring| spring.rest(0.5, 5.0))
        .expect("a positive duration makes a valid spring");
    Transition::Spring(spring)
}

/// Whether motion snaps to its end state (the OS "reduce motion" setting).
pub fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}

/// Follow the OS "reduce motion" setting: `initial` now, then each value
/// `changes` yields, for as long as the app runs.
pub fn follow_reduce_motion(
    initial: bool,
    changes: impl Stream<Item = bool> + 'static,
    cx: &mut App,
) {
    cx.set_reduce_motion(initial);
    cx.spawn(async move |cx| {
        let mut changes = std::pin::pin!(changes);
        while let Some(reduce) = changes.next().await {
            cx.update(|cx| cx.set_reduce_motion(reduce));
        }
    })
    .detach();
}

/// Motion state kept in element state under `id` while the element renders
/// in consecutive frames (`Window::use_keyed_state`): for stateless
/// components. The rendering view re-renders when the state notifies.
pub fn use_motion<S: 'static>(
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
    init: impl FnOnce() -> S,
) -> Entity<S> {
    window.use_keyed_state(id, cx, |_, _| init())
}

/// The presented value of a number that eases to `target` over `tween`
/// whenever `target` changes. The first render shows `target` as is (nothing
/// animates on mount); under reduced motion it always does.
pub fn animate(
    id: impl Into<ElementId>,
    target: f32,
    tween: Tween,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let state = use_motion(id, window, cx, || ValueState::new(f64::from(target)));
    state.update(cx, |state, cx| {
        state
            .frame(f64::from(target), tween.transition(), window, cx)
            .value() as f32
    })
}

/// 0 ⇄ 1 progress of a two-state control (a switch, a disclosure), easing
/// over `tween` after each flip.
pub fn progress(
    id: impl Into<ElementId>,
    on: bool,
    tween: Tween,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    animate(id, if on { 1. } else { 0. }, tween, window, cx)
}

/// A hover-driven value for one element: [`HoverMotion::set`] from the
/// element's `on_hover`, read with [`hover`].
pub struct HoverMotion {
    hovered: bool,
    value: ValueState,
}

impl HoverMotion {
    /// Record whether the pointer is over the element; the view re-renders.
    pub fn set(state: &Entity<Self>, hovered: bool, cx: &mut App) {
        state.update(cx, |state, cx| {
            if state.hovered != hovered {
                state.hovered = hovered;
                cx.notify();
            }
        });
    }
}

/// The 0 → 1 hover progress of the element keyed `id`, retargeting mid-flight,
/// and the state its `on_hover` must feed ([`HoverMotion::set`]). At rest it
/// requests no frames.
pub fn hover(
    id: impl Into<ElementId>,
    tween: Tween,
    window: &mut Window,
    cx: &mut App,
) -> (f32, Entity<HoverMotion>) {
    let state = use_motion(id, window, cx, || HoverMotion {
        hovered: false,
        value: ValueState::new(0.),
    });
    let value = state.update(cx, |state, cx| {
        let target = if state.hovered { 1. } else { 0. };
        state
            .value
            .frame(target, tween.transition(), window, cx)
            .value() as f32
    });
    (value, state)
}

/// `a` → `b` at `t` (0..=1), mixed in RGB like a CSS colour transition.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    if t <= 0. {
        return a;
    }
    if t >= 1. {
        return b;
    }
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Hsla::from(Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    })
}

/// A popover, menu or dialog panel appearing: opacity 0 → 1 while it slides
/// `from_y` px into place over [`FADE`]. It takes input from its first frame
/// and leaves at once when closed (no exit animation holds focus).
pub fn appear(from_y: f32) -> Motion {
    Motion::new(Target::default())
        .initial(Target::default().y(from_y).opacity(0.))
        .transition(FADE.transition())
}

/// [`appear`] for the element keyed `id`: its frame, restarting each time the
/// element mounts again (a menu reopened).
pub fn appear_frame(
    id: impl Into<ElementId>,
    from_y: f32,
    window: &mut Window,
    cx: &mut App,
) -> Frame {
    let state = use_motion(id, window, cx, MotionState::default);
    state.update(cx, |state, cx| state.frame(appear(from_y), window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn presets_ease_like_css() {
        let mut value = gpui_motion_kit::core::MotionValue::new(0.);
        value.animate(100., CONTROL.transition(), Duration::ZERO);
        // CSS `ease` is past 80% at the halfway point; gpui-motion's default
        // ease-out would be at 87.5%.
        let half = value.sample(MS(75), false).value();
        assert!((80. ..82.).contains(&half), "{half}");
        let end = value.sample(CONTROL.duration, false);
        assert_eq!(end.value(), 100.);
        assert!(!end.is_animating());
    }

    #[test]
    fn reduced_motion_snaps() {
        let mut value = gpui_motion_kit::core::MotionValue::new(0.);
        value.animate(16., CONTROL.transition(), Duration::ZERO);
        let frame = value.sample(MS(1), true);
        assert_eq!(frame.value(), 16.);
        assert!(!frame.is_animating());

        let mut state = MotionState::default();
        let frame = state.sample(appear(-4.), Duration::ZERO, true);
        assert_eq!((frame.target().y_px(), frame.target().alpha()), (0., 1.));
        assert!(!frame.is_animating());
    }

    #[test]
    fn appear_starts_hidden_and_settles() {
        let mut state = MotionState::default();
        let first = state.sample(appear(-4.), Duration::ZERO, false);
        assert_eq!((first.target().y_px(), first.target().alpha()), (-4., 0.));
        assert!(first.is_present() && first.is_animating());
        let settled = state.sample(appear(-4.), FADE.duration, false);
        assert_eq!(
            (settled.target().y_px(), settled.target().alpha()),
            (0., 1.)
        );
        assert!(!settled.is_animating());
    }

    #[test]
    fn release_spring_rests() {
        let mut value = gpui_motion_kit::core::MotionValue::new(300.);
        value.animate(0., release_spring(SHEET_SNAP.duration), Duration::ZERO);
        let frame = value.sample(Duration::from_secs(2), false);
        assert_eq!(frame.value(), 0.);
        assert!(!frame.is_animating());
    }

    #[test]
    fn mix_ends_exactly() {
        let (a, b) = (
            gpui_kit::hsla(0., 0., 0., 1.),
            gpui_kit::hsla(0.5, 1., 0.5, 1.),
        );
        assert_eq!(mix(a, b, 0.), a);
        assert_eq!(mix(a, b, 1.), b);
        let mid = Rgba::from(mix(a, b, 0.5));
        assert!((mid.g - 0.5).abs() < 0.01, "{mid:?}");
    }
}
