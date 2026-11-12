//! More tabs than the window is wide: the strip scrolls sideways to the last
//! one.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use roostr_core::nav::Route;

use common::{Harness, OnePage};

const W: f32 = 900.0;

#[test]
fn too_many_tabs_scroll_sideways() {
    let mut h = Harness::new(Arc::new(OnePage), (W, 700.0));
    h.settle();
    // Twenty different pages: one page opens one tab.
    for i in 0..20 {
        h.update(|s| roostr_core::nav::open_tab(Route::Object(format!("page-{i}")), false, s));
    }
    h.settle();
    let tabs = h.keys("tab:");
    assert!(tabs.len() >= 20, "{tabs:?}");
    let last = tabs.last().unwrap().clone();
    let first = tabs.first().unwrap().clone();

    let strip = h.rect("tab-strip-scroll").expect("the scrolling strip");
    assert!(strip[0] + strip[2] <= W, "the strip ends in the window");

    // The last opened tab is active; scroll to the start, then the end.
    let at = |h: &Harness, k: &str| h.rect(k).unwrap()[0];
    let swipe = |h: &mut Harness, dx: f32| {
        let (x, y) = h.rt.find("tab-strip-scroll").unwrap();
        h.rt.pointer_move(x, y);
        h.rt.wheel_xy(x, y, dx, 0.0);
        h.frames(30);
    };
    swipe(&mut h, 20_000.0);
    let a = at(&h, &first);
    swipe(&mut h, -20_000.0);
    let b = at(&h, &first);
    assert!(a >= strip[0], "at the start the first tab shows: {a}");
    let end = h.rect(&last).unwrap();
    assert!(
        end[0] + end[2] <= strip[0] + strip[2] + 0.5 && end[0] >= strip[0],
        "the last tab scrolls into the strip: {end:?} in {strip:?}"
    );
    assert!(b < strip[0], "and the first scrolled off: {b}");

    // A mouse wheel (up and down) over the strip scrolls it sideways too: back to the start.
    let wheel = |h: &mut Harness, dy: f32| {
        let (x, y) = h.rt.find("tab-strip-scroll").unwrap();
        h.rt.pointer_move(x, y);
        for _ in 0..40 {
            h.rt.wheel_xy(x, y, 0.0, dy);
            h.frames(2);
        }
        h.frames(10);
    };
    wheel(&mut h, 500.0);
    assert!(
        at(&h, &first) >= strip[0],
        "the wheel scrolled back to the first tab"
    );
    wheel(&mut h, -500.0);
    let end = h.rect(&last).unwrap();
    assert!(
        end[0] + end[2] <= strip[0] + strip[2] + 0.5,
        "and on to the last: {end:?} in {strip:?}"
    );
}
