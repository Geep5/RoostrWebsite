//! The phone shell's push/pop motion and the iOS back swipe.
//!
//! The phone's screens stack: the Spaces list, a space's home, an object
//! (or its full-screen chat). Going back slides the screen off to the right
//! over its parent, which rides in from a little left of the edge and
//! brightens as it is uncovered; opening a screen from its parent slides it
//! in from the right the same way. On iOS a pan from the screen's left edge
//! drags the screen with the finger (UINavigationController's interactive
//! pop): let go a third of the way across, or flicked right, and it goes
//! back; otherwise it springs back. Both settle on [`SETTLE`].
//!
//! Motion runs on touch screens only (a narrow desktop window swaps screens
//! at once) and never under reduced motion: there a swipe still goes back
//! on release, but the screen does not follow the finger.
//!
//! The swipe is iOS-only. Mobile Safari, and Android, already take a pan
//! from the left edge as their own back gesture (browser history, which the
//! app's routes follow), so the web shell leaves the edge to them.

use std::time::Duration;

use gpui_kit::{Pixels, Point};
use roostr_core::motion::ValueState;
use web_time::Instant;

/// How far in from the left edge a back swipe may start (UIKit's ~20pt).
pub(crate) const EDGE: f32 = 20.;
/// A release past this fraction of the width goes back.
const COMMIT_FRACTION: f32 = 1. / 3.;
/// A release moving faster than this (px/s) decides by its direction alone.
const FLICK: f32 = 300.;
/// The window of finger samples the release velocity is measured over.
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);
/// A finger still this long before release has stopped: no flick.
const STOPPED: Duration = Duration::from_millis(60);
/// The push/pop and release spring: 90% of the way in this long.
pub(crate) const SETTLE: Duration = Duration::from_millis(300);
/// How far left of its place the parent screen starts (UIKit's 30%).
pub(crate) const PARALLAX: f32 = 0.3;
/// The parent screen's dimming while fully covered.
pub(crate) const DIM: f32 = 0.12;

/// Whether a touch pan that started at `start` and first moved `delta`
/// (past the slop) is a back swipe: from the left edge, rightward, and more
/// sideways than vertical.
pub(crate) fn claims(start: Point<Pixels>, delta: Point<Pixels>) -> bool {
    let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
    f32::from(start.x) <= EDGE && dx > 0. && dx > dy.abs()
}

/// Whether a swipe released `offset` px across a `width` px screen, moving
/// at `velocity` px/s (positive = rightward), goes back.
pub(crate) fn commits(offset: f32, width: f32, velocity: f32) -> bool {
    if velocity >= FLICK {
        true
    } else if velocity <= -FLICK {
        false
    } else {
        offset >= width * COMMIT_FRACTION
    }
}

/// A back swipe under the finger: how far it has dragged the screen, and
/// recent samples for the release velocity.
#[derive(Debug, Default)]
pub(crate) struct Track {
    pub(crate) offset: f32,
    samples: Vec<(Instant, f32)>,
}

impl Track {
    /// The finger moved `dx` px at `now`.
    pub(crate) fn push(&mut self, now: Instant, dx: f32) {
        self.offset = (self.offset + dx).max(0.);
        self.samples.push((now, self.offset));
        self.samples
            .retain(|(at, _)| now.saturating_duration_since(*at) <= VELOCITY_WINDOW);
    }

    /// The finger's rightward speed (px/s) at `now`: 0 once it has stopped.
    pub(crate) fn velocity(&self, now: Instant) -> f32 {
        let (Some(&(first_at, first)), Some(&(last_at, last))) =
            (self.samples.first(), self.samples.last())
        else {
            return 0.;
        };
        let span = last_at.saturating_duration_since(first_at).as_secs_f32();
        if now.saturating_duration_since(last_at) > STOPPED || span <= 0. {
            return 0.;
        }
        (last - first) / span
    }
}

/// A phone screen, as the stack sees it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Screen {
    Spaces,
    /// A space's home (its id).
    Home(String),
    Object(String),
    Chat(String),
    /// Off the stack (the graph): no swipe, no slide.
    Other,
}

#[derive(Debug, Default)]
pub(crate) enum Phase {
    #[default]
    Idle,
    /// A back swipe follows the finger.
    Tracking(Track),
    /// Sliding off to the right, going back when it gets there.
    Popping,
    /// Off the right edge: the parent is about to show.
    Popped,
    /// Springing back after a swipe let go short.
    Returning,
    /// Sliding in from the right over the screen it was opened from.
    Pushing(Screen),
}

/// The phone shell's stack motion.
pub(crate) struct BackSwipe {
    pub(crate) phase: Phase,
    /// The top screen's offset from its place (px, rightward).
    pub(crate) offset: ValueState,
    /// The screen the last frame showed.
    pub(crate) shown: Option<Screen>,
    /// A released swipe's fling momentum is still arriving: eat it.
    pub(crate) swallow: bool,
}

impl Default for BackSwipe {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            offset: ValueState::new(0.),
            shown: None,
            swallow: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px};

    #[test]
    fn claims_only_rightward_pans_from_the_left_edge() {
        let edge = point(px(8.), px(400.));
        assert!(claims(edge, point(px(12.), px(3.))));
        assert!(claims(point(px(EDGE), px(400.)), point(px(9.), px(0.))));
        // Away from the edge: the page's (a table strip, the editor).
        assert!(!claims(point(px(40.), px(400.)), point(px(12.), px(0.))));
        // Leftward, or mostly vertical: a scroll.
        assert!(!claims(edge, point(px(-12.), px(0.))));
        assert!(!claims(edge, point(px(6.), px(10.))));
        assert!(!claims(edge, point(px(6.), px(-10.))));
        // A caught fling starts with no movement.
        assert!(!claims(edge, point(px(0.), px(0.))));
    }

    #[test]
    fn commits_past_a_third_or_on_a_flick() {
        let width = 390.;
        assert!(commits(140., width, 0.));
        assert!(!commits(120., width, 0.));
        // A rightward flick goes back from anywhere; a leftward one stays.
        assert!(commits(30., width, 600.));
        assert!(!commits(300., width, -600.));
        // A slow drift decides by distance.
        assert!(!commits(100., width, 200.));
        assert!(commits(200., width, -200.));
    }

    #[test]
    fn track_follows_the_finger_and_measures_its_speed() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut track = Track::default();
        track.push(at(0), 10.);
        track.push(at(16), 8.);
        track.push(at(32), 8.);
        assert_eq!(track.offset, 26.);
        // 16 px over 32 ms.
        assert!((track.velocity(at(40)) - 500.).abs() < 1.);
        // Held still before letting go: no flick.
        assert_eq!(track.velocity(at(200)), 0.);
        // Dragged back past the edge: pinned at 0.
        track.push(at(48), -100.);
        assert_eq!(track.offset, 0.);
        assert!(track.velocity(at(50)) < 0.);
    }
}
