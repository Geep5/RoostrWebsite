//! Motion: the Svelte app's CSS timings as Rune curves and springs, plus Roostr's own touches
//! (press scale, bouncy popups, rolling numbers). Every UI crate animates through
//! this module.
//!
//! - Tweens match the Svelte CSS (`transition: … 0.15s ease`); CSS `ease` is
//!   `cubic-bezier(0.25, 0.1, 0.25, 1)`, not an ease-out.
//! - Springs settle a gesture release ([`release_spring`]) and open popups, dialogs and sheets
//!   with a slight bounce ([`POP`], [`SHEET_OPEN`]); their exits stay instant.
//! - Tappable things shrink while held ([`pressable`]).
//! - Changing counts roll their digits ([`rolling_count`], [`rolling_text`]).
//! - Looping indicators (sync pulse, typing dots, spinners) use `El::looping`.
//! - Reduced motion is Rune's: spatial travel snaps and opacity, presence and colour take a
//!   short fade (`REDUCED_FADE`); press scale is off and rolling digits crossfade. [`reduced`]
//!   reports it for code that picks other motion.
//! - Motion never gates input: menus, dialogs and sheets take focus and keys at once, and
//!   anything interactive leaves without an exit animation holding focus.
//!
//! Where an API takes an `Anim` (`El::opacity_motion`, `El::offset_motion`, `El::hover_motion`,
//! `Tx::motion_curve`) pass [`Tween::anim`]; where it takes a `Spring` (`El::motion`,
//! `Tx::color_motion`) pass [`Tween::spring`], a zero-bounce spring of the same length.

use rune::{Anim, Color, Curve, Easing, Effect, El, Spring, SpringPair, TextStyle, Transition};
use rune_kit::RollingText;

use crate::Theme;

/// CSS `ease`: cubic-bezier(0.25, 0.1, 0.25, 1).
const EASE: [f32; 4] = [0.25, 0.1, 0.25, 1.0];
/// The rail handle's cubic-bezier(0.22, 1, 0.36, 1).
const EASE_OUT_QUINT: [f32; 4] = [0.22, 1.0, 0.36, 1.0];

/// A duration and cubic-bezier curve: one of the Svelte app's CSS transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    /// Seconds.
    pub duration: f32,
    curve: [f32; 4],
}

impl Tween {
    /// `ms` milliseconds of CSS `ease`.
    pub const fn ease(ms: u32) -> Self {
        Self::bezier(ms, EASE)
    }

    /// `ms` milliseconds of cubic-bezier(`curve`).
    pub const fn bezier(ms: u32, curve: [f32; 4]) -> Self {
        Self {
            duration: ms as f32 / 1000.0,
            curve,
        }
    }

    /// The same curve over `seconds`.
    pub const fn with_duration(self, seconds: f32) -> Self {
        Self {
            duration: seconds,
            ..self
        }
    }

    pub const fn easing(self) -> Easing {
        let [x1, y1, x2, y2] = self.curve;
        Easing::cubic(x1, y1, x2, y2)
    }

    /// The tween as a Rune curve (CSS transition semantics: retargets add).
    pub const fn curve(self) -> Curve {
        Curve::tween(self.duration, self.easing())
    }

    /// The tween as an animation, for `El::opacity_motion`, `El::hover_motion`, `Tx::motion_curve`.
    pub const fn anim(self) -> Anim {
        Anim::new(self.curve())
    }

    /// A zero-bounce spring as long as the tween, for APIs that take only springs
    /// (`El::motion`, `El::token_motion`, `Tx::color_motion`).
    pub const fn spring(self) -> Spring {
        Spring::new(self.duration, 0.0)
    }
}

impl From<Tween> for Anim {
    fn from(t: Tween) -> Anim {
        t.anim()
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

/// The theme switch's colour spring (`Tx::color_motion`): Rune's calm token, as its own
/// settings window switches appearance.
pub const THEME_SWITCH: Spring = Spring::new(0.4, 0.0);

/// Menus, popovers, selects and dialogs opening: quick, with a slight bounce past their place.
pub const POP: Spring = Spring::new(0.3, 0.15);
/// A sheet rising open (the phone sheets, the settings sheet): a little longer than [`POP`],
/// with the same slight bounce as it lands.
pub const SHEET_OPEN: Spring = Spring::new(0.38, 0.12);
/// How far a held tappable shrinks ([`pressable`]).
pub const PRESS_SCALE: f32 = 0.97;
/// How far a held full-width row shrinks ([`pressable_row`]): a wide row at [`PRESS_SCALE`]
/// would pull its ends in by several points, so rows dip by about the same few points as a
/// card does.
pub const ROW_PRESS_SCALE: f32 = 0.985;
/// How small a popup starts as it pops open ([`pop`]).
const POP_SCALE: f32 = 0.96;

/// CSS `ease`, the Svelte app's default transition curve.
pub const fn css_ease() -> Easing {
    COLUMNS.easing()
}

/// Settling after a gesture lets go (a sheet flung open or shut): critically damped, settled
/// in about `seconds`.
pub const fn release_spring(seconds: f32) -> Spring {
    Spring::new(seconds, 0.0)
}

/// Whether motion snaps to its end state this commit (the OS "reduce motion" setting, or the
/// view's override).
pub fn reduced<S>(cx: &rune::Cx<S>) -> bool {
    cx.is_reduced_motion()
}

/// A popover, menu or dialog panel appearing: opacity 0 → 1 and a slight scale-up while it
/// slides `from_y` points into place, on [`POP`] (it overshoots a touch and settles). It takes
/// input from its first frame and leaves at once when closed (no exit animation holds focus).
/// Put it on the panel: `.transition(motion::appear(-4.0))`.
pub fn appear(from_y: f32) -> Transition {
    pop(Effect {
        offset: rune::Vec2::new(0.0, from_y),
        // Grows out of the side it comes from: the top when it drops, the bottom when it rises.
        anchor: rune::Vec2::new(0.5, if from_y > 0.0 { 1.0 } else { 0.0 }),
        ..Effect::scale(POP_SCALE).combined(Effect::opacity(0.0))
    })
}

/// `insertion` springing to rest on [`POP`], gone at once on removal: the one entrance every
/// Roostr popup, dialog and editor panel opens with.
pub fn pop(insertion: Effect) -> Transition {
    Transition {
        insertion,
        removal: Effect::IDENTITY,
        springs: SpringPair::new(POP, Spring::NONE),
        ..Transition::identity()
    }
}

/// A tappable element (row, card, tile, pill, menu item, button) that shrinks to
/// [`PRESS_SCALE`] about its centre while a press on it is held and springs back on release.
/// Layout and hit testing keep its resting size; Reduce Motion turns it off.
pub fn pressable(e: El) -> El {
    e.press_scale(PRESS_SCALE)
}

/// [`pressable`] for a full-width row (a list, sidebar or table row), on [`ROW_PRESS_SCALE`].
pub fn pressable_row(e: El) -> El {
    e.press_scale(ROW_PRESS_SCALE)
}

/// A count whose changed digits roll to the new value (up as it rises, down as it falls);
/// the first build shows it as is. `key` names the label and must be unique in the window.
pub fn rolling_count(
    t: &Theme,
    key: impl Into<String>,
    n: usize,
    font: TextStyle,
    color: Color,
) -> El {
    rolling_text(t, key, n.to_string(), n as f64, font, color)
}

/// [`rolling_count`] for a label carrying a number (`"7 objects"`, `"42%"`): the characters that
/// changed roll, in the direction `value` moved.
pub fn rolling_text(
    t: &Theme,
    key: impl Into<String>,
    text: impl Into<String>,
    value: f64,
    font: TextStyle,
    color: Color,
) -> El {
    RollingText::new(key, text)
        .value(value)
        .font(font)
        .color(color)
        .build(&t.rk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_ease_like_css() {
        // CSS `ease` is past 80% at the halfway point; an ease-out would be at 87.5%.
        let half = CONTROL.easing().eval(0.5);
        assert!((0.80..0.82).contains(&half), "{half}");
        assert_eq!(CONTROL.easing().eval(1.0), 1.0);
        assert_eq!(CONTROL.duration, 0.15);
        assert_eq!(HANDLE.duration, 0.15);
        assert_eq!(DISCLOSURE.duration, 0.2);
    }

    #[test]
    fn tweens_become_rune_curves() {
        match FADE.curve() {
            Curve::Tween { duration, .. } => assert_eq!(duration, 0.12),
            other => panic!("{other:?}"),
        }
        assert_eq!(SHEET_SNAP.spring().duration, 0.18);
    }
}
