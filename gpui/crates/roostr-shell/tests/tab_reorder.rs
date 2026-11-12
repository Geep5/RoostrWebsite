//! Dragging a tab along the strip: it follows the pointer, the tabs it passes
//! step aside to open its place, and letting go settles it there without a
//! jump. A press that dragged doesn't switch tabs.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use roostr_core::nav::Route;

use common::{Harness, OnePage};

fn order(h: &Harness) -> Vec<String> {
    h.state()
        .nav
        .tabs()
        .iter()
        .map(|t| t.route.path())
        .collect()
}

#[test]
fn a_dragged_tab_follows_the_pointer_and_its_neighbours_step_aside() {
    let mut h = Harness::new(Arc::new(OnePage), (1200.0, 700.0));
    h.settle();
    for id in ["a", "b", "c"] {
        h.update(|s| roostr_core::nav::open_tab(Route::Object(id.into()), false, s));
    }
    h.update(|s| s.nav.activate(0, &mut s.ui));
    h.settle();
    let uids: Vec<u64> = h.state().nav.tabs().iter().map(|t| t.uid).collect();
    let slot = |i: usize| format!("tab-slot:{}", uids[i]);
    let start: Vec<[f32; 4]> = (0..4).map(|i| h.rect(&slot(i)).unwrap()).collect();

    // Grab tab `a` (second) and carry it past `c` (last), a step at a time.
    let grab = h.rect(&format!("tab:{}", uids[1])).unwrap();
    let (x, y) = (grab[0] + grab[2] / 2., grab[1] + grab[3] / 2.);
    let travel = start[3][0] + start[3][2] - (start[1][0] + start[1][2]);
    h.rt.pointer_move(x, y);
    h.rt.pointer_down(x, y);
    for step in 1..=10 {
        h.rt.pointer_move(x + travel * step as f32 / 10., y);
        h.frames(2);
    }
    h.frames(30);
    let held = h.rect(&slot(1)).unwrap();
    assert!(
        (held[0] - (start[1][0] + travel)).abs() < 1.,
        "the dragged tab is under the pointer: {held:?}, from {:?}",
        start[1]
    );
    for i in [2, 3] {
        let r = h.rect(&slot(i)).unwrap();
        assert!(
            r[0] < start[i][0] - start[1][2] * 0.9,
            "tab {i} stepped aside to the left: {r:?}, was {:?}",
            start[i]
        );
    }
    assert_eq!(h.rect(&slot(0)).unwrap()[0], start[0][0], "home stays put");
    assert_eq!(order(&h), ["/app", "/app/object/a", "/app/object/b", "/app/object/c"]);

    // Let go: the strip's order changes and the tab doesn't jump.
    h.rt.pointer_up(x + travel, y);
    h.frames(1);
    assert_eq!(order(&h), ["/app", "/app/object/b", "/app/object/c", "/app/object/a"]);
    let moved = h.state().nav.tabs()[3].uid;
    let landed = h.rect(&format!("tab-slot:{moved}")).unwrap();
    assert!(
        (landed[0] - held[0]).abs() < 4.,
        "the dropped tab stays where it was let go: {landed:?} vs {held:?}"
    );
    h.settle();
    let end = h.rect(&format!("tab-slot:{moved}")).unwrap();
    assert!((end[0] + end[2] - (start[3][0] + start[3][2])).abs() < 1., "{end:?}");
    assert_eq!(h.state().nav.active_index(), 0, "the drag switched no tab");
}
