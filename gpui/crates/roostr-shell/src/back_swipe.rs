//! The phone shell's push/pop motion and the edge back swipe.
//!
//! The phone's screens stack: the Spaces list, a space's home, an object
//! (or its full-screen chat). Going back slides the screen off to the right
//! over its parent, which rides in from a little left of the edge and
//! brightens as it is uncovered; opening a screen from its parent slides it
//! in from the right the same way. A pan from the screen's left edge drags
//! the screen with the finger (UINavigationController's interactive pop):
//! let go a third of the way across, or flicked right, and it goes back;
//! otherwise it springs back. Both settle on [`SETTLE`].
//!
//! Motion runs on touch screens only (the browser build at a phone width;
//! a narrow desktop window swaps screens at once) and never under reduced
//! motion: there a swipe still goes back on release, but the screen does not
//! follow the finger.

use std::time::Duration;

use roostr_rt::Task;
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
/// When a push, pop or return has come to rest (the spring's tail included).
pub(crate) const SETTLED: Duration = Duration::from_millis(500);
/// How far left of its place the parent screen starts (UIKit's 30%).
pub(crate) const PARALLAX: f32 = 0.3;
/// The parent screen's dimming while fully covered.
pub(crate) const DIM: f32 = 0.12;

/// Whether a touch pan that started at `start` (window points) and first
/// moved `delta` (past the slop) is a back swipe: from the left edge,
/// rightward, and more sideways than vertical.
pub(crate) fn claims(start: [f32; 2], delta: [f32; 2]) -> bool {
    let [dx, dy] = delta;
    start[0] <= EDGE && dx > 0. && dx > dy.abs()
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
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// A space's open tabs (its id), over its home.
    Tabs(String),
    Object(String),
    Chat(String),
    /// Off the stack (the graph): no swipe, no slide.
    Other,
}

impl Screen {
    /// The key the screen's element keeps while it is shown, on top or under
    /// the one above it, so a screen keeps its place (and its motion) as it
    /// moves between the two.
    pub(crate) fn key(&self) -> String {
        match self {
            Screen::Spaces => "m-screen-spaces".into(),
            Screen::Home(id) => format!("m-screen-home-{id}"),
            Screen::Tabs(id) => format!("m-screen-tabs-{id}"),
            Screen::Object(id) => format!("m-screen-object-{id}"),
            Screen::Chat(id) => format!("m-screen-chat-{id}"),
            Screen::Other => "m-screen-other".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Phase {
    #[default]
    Idle,
    /// A back swipe follows the finger.
    Tracking(Track),
    /// A back button was pressed: the parent shows under the screen, which
    /// starts sliding off on the next commit.
    Arming,
    /// Sliding off to the right, going back when it gets there.
    Popping,
    /// Off the right edge: the parent is about to show.
    Popped,
    /// Springing back after a swipe let go short.
    Returning,
    /// Sliding in from the right over the screen it was opened from.
    Pushing(Screen),
}

impl Phase {
    /// The top screen's offset from its place (px, rightward) on a `width`
    /// screen; `follow` false (reduced motion) keeps a tracked swipe still.
    pub(crate) fn offset(&self, width: f32, follow: bool) -> f32 {
        match self {
            Phase::Idle | Phase::Arming | Phase::Returning | Phase::Pushing(_) => 0.,
            Phase::Tracking(_) if !follow => 0.,
            Phase::Tracking(track) => track.offset.min(width),
            Phase::Popping | Phase::Popped => width,
        }
    }

    /// Whether the screen under the top one shows.
    pub(crate) fn shows_under(&self) -> bool {
        !matches!(self, Phase::Idle)
    }
}

/// The phone shell's stack motion.
#[derive(Default)]
pub(crate) struct BackSwipe {
    pub(crate) phase: Phase,
    /// The screen the last poll saw shown.
    pub(crate) shown: Option<Screen>,
    /// Ends the running phase when it has settled (dropping it cancels).
    pub(crate) timer: Option<Task<()>>,
}

/// What a newly shown `screen` does, the one before it `prev`: slide in over
/// it when it was opened from it (`parent` of the new screen is `prev`, and
/// `motion` is on), else nothing (a pop landing, a jump).
pub(crate) fn observe(prev: &Screen, parent: Option<&Screen>, motion: bool) -> Phase {
    if motion && parent == Some(prev) {
        Phase::Pushing(prev.clone())
    } else {
        Phase::Idle
    }
}

/// The screen showing under the top one while it moves: the screen it was
/// pushed over, else its parent.
pub(crate) fn under(phase: &Phase, parent: Option<Screen>) -> Option<Screen> {
    match phase {
        Phase::Idle => None,
        Phase::Pushing(from) => Some(from.clone()),
        _ => parent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_only_rightward_pans_from_the_left_edge() {
        let edge = [8., 400.];
        assert!(claims(edge, [12., 3.]));
        assert!(claims([EDGE, 400.], [9., 0.]));
        // Away from the edge: the page's (a table strip, the editor).
        assert!(!claims([40., 400.], [12., 0.]));
        // Leftward, or mostly vertical: a scroll.
        assert!(!claims(edge, [-12., 0.]));
        assert!(!claims(edge, [6., 10.]));
        assert!(!claims(edge, [6., -10.]));
        // A caught fling starts with no movement.
        assert!(!claims(edge, [0., 0.]));
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

    #[test]
    fn a_screen_opened_from_its_parent_slides_in_over_it() {
        let home = Screen::Home("sp".into());
        let object = Screen::Object("o".into());
        assert_eq!(
            observe(&home, Some(&home), true),
            Phase::Pushing(home.clone())
        );
        // A jump, a pop landing, or no motion: no slide.
        assert_eq!(observe(&object, Some(&home), true), Phase::Idle);
        assert_eq!(observe(&home, Some(&home), false), Phase::Idle);
        // The pushed-over screen shows under; otherwise the parent does.
        assert_eq!(
            under(&Phase::Pushing(Screen::Spaces), Some(home.clone())),
            Some(Screen::Spaces)
        );
        assert_eq!(under(&Phase::Popping, Some(home.clone())), Some(home));
        assert_eq!(under(&Phase::Idle, Some(Screen::Spaces)), None);
    }

    #[test]
    fn the_top_screen_rides_the_phase() {
        let mut track = Track::default();
        track.push(Instant::now(), 500.);
        assert_eq!(Phase::Tracking(track.clone()).offset(390., true), 390.);
        assert_eq!(Phase::Tracking(track).offset(390., false), 0.);
        assert_eq!(Phase::Popping.offset(390., true), 390.);
        assert_eq!(Phase::Returning.offset(390., true), 0.);
        assert_eq!(Phase::Arming.offset(390., true), 0.);
    }
}
