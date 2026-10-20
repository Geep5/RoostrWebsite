//! roostr: a wheel or trackpad gesture's scroll rails (`gpui::WheelRails`),
//! and Shift turning a mouse wheel sideways (`gpui::wheel_scroll_delta`).

use std::time::{Duration, Instant};

use gpui::{
    Axis, Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, WheelRails, point, px,
    wheel_scroll_delta,
};

/// Feeds `rails` one event every 16ms (a browser's trackpad stream: no
/// phases), returning each event's rail.
fn rail_steps(rails: &mut WheelRails, start: Instant, steps: &[(f32, f32)]) -> Vec<Option<Axis>> {
    steps
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let at = start + Duration::from_millis(16 * i as u64);
            rails.rail(point(px(x), px(y)), TouchPhase::Moved, at)
        })
        .collect()
}

#[test]
fn a_mostly_vertical_trackpad_gesture_runs_on_the_vertical_rail() {
    let mut rails = WheelRails::default();
    // Down and drifting right, one pixel in four.
    let rail = rail_steps(&mut rails, Instant::now(), &[(1., 4.); 12]);
    assert!(rail.iter().all(|r| *r == Some(Axis::Vertical)), "{rail:?}");
}

#[test]
fn a_mostly_horizontal_trackpad_gesture_runs_on_the_horizontal_rail() {
    let mut rails = WheelRails::default();
    let rail = rail_steps(&mut rails, Instant::now(), &[(-5., 1.); 12]);
    assert!(
        rail.iter().all(|r| *r == Some(Axis::Horizontal)),
        "{rail:?}"
    );
}

#[test]
fn a_diagonal_trackpad_gesture_runs_free() {
    let mut rails = WheelRails::default();
    let rail = rail_steps(&mut rails, Instant::now(), &[(3., 4.); 12]);
    // The first steps, short of a 12px segment, keep to their own axis.
    assert_eq!(rail[..2], [Some(Axis::Vertical); 2]);
    assert!(rail[2..].iter().all(|r| r.is_none()), "{rail:?}");
}

#[test]
fn a_gesture_that_turns_sideways_switches_rails_and_a_wobble_does_not() {
    let now = Instant::now();
    let mut rails = WheelRails::default();
    rail_steps(&mut rails, now, &[(0., 6.); 6]);
    // A diagonal wobble keeps the vertical rail.
    let at = now + Duration::from_millis(100);
    assert_eq!(
        rail_steps(&mut rails, at, &[(5., 6.); 4]),
        [Some(Axis::Vertical); 4]
    );
    // A clear turn sideways takes the horizontal rail after a segment.
    let at = now + Duration::from_millis(170);
    let rail = rail_steps(&mut rails, at, &[(6., 0.); 4]);
    assert_eq!(rail[0], Some(Axis::Vertical));
    assert_eq!(rail[1..], [Some(Axis::Horizontal); 3]);
}

#[test]
fn a_pause_or_a_new_phase_starts_a_new_gesture() {
    let now = Instant::now();
    let mut rails = WheelRails::default();
    rail_steps(&mut rails, now, &[(0., 20.)]);
    let after_pause = now + Duration::from_millis(200);
    assert_eq!(
        rails.rail(point(px(20.), px(0.)), TouchPhase::Moved, after_pause),
        Some(Axis::Horizontal)
    );
    let soon = after_pause + Duration::from_millis(1);
    assert_eq!(
        rails.rail(point(px(2.), px(5.)), TouchPhase::Started, soon),
        Some(Axis::Vertical)
    );
}

#[test]
fn momentum_after_the_fingers_lift_keeps_the_rail() {
    // macOS: Began, moves, Ended, then momentum events without a phase.
    let now = Instant::now();
    let mut rails = WheelRails::default();
    rails.rail(point(px(0.), px(0.)), TouchPhase::Started, now);
    rail_steps(&mut rails, now, &[(1., 8.); 4]);
    let lift = now + Duration::from_millis(64);
    rails.rail(point(px(0.), px(0.)), TouchPhase::Ended, lift);
    let coast = lift + Duration::from_millis(16);
    assert_eq!(
        rail_steps(&mut rails, coast, &[(2., 5.); 6]),
        [Some(Axis::Vertical); 6]
    );
}

#[test]
fn shift_turns_a_mouse_wheel_sideways_but_not_a_trackpad() {
    let wheel = |delta, shift| ScrollWheelEvent {
        delta,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
        ..ScrollWheelEvent::default()
    };
    let line = px(20.);
    let notch = ScrollDelta::Lines(point(0., -3.));
    assert_eq!(
        wheel_scroll_delta(&wheel(notch, true), line),
        point(px(-60.), px(0.))
    );
    assert_eq!(
        wheel_scroll_delta(&wheel(notch, false), line),
        point(px(0.), px(-60.))
    );
    // Already sideways (macOS and Chrome turn a shifted wheel themselves).
    let turned = ScrollDelta::Lines(point(-3., 0.));
    assert_eq!(
        wheel_scroll_delta(&wheel(turned, true), line),
        point(px(-60.), px(0.))
    );
    let swipe = ScrollDelta::Pixels(point(px(0.), px(-8.)));
    assert_eq!(
        wheel_scroll_delta(&wheel(swipe, true), line),
        point(px(0.), px(-8.))
    );
}
