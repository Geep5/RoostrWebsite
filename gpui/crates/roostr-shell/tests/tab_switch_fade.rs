//! Switching tabs moves the active tab's look across to the picked tab (it slides, rather
//! than jumping), and the new page fades up while the old one is gone at once.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use roostr_core::nav::Route;

use common::{Harness, OnePage};

/// The page keyed by route `path`: (exiting, opacity) of each copy.
fn page(h: &Harness, path: &str) -> Vec<(bool, f32)> {
    let key = format!("main-scroll:{path}");
    h.rt.elements()
        .into_iter()
        .filter(|e| e.key.as_deref() == Some(key.as_str()))
        .map(|e| (e.exiting, e.opacity))
        .collect()
}

#[test]
fn switching_tabs_slides_the_active_look_and_fades_the_page_in() {
    let mut h = Harness::new(Arc::new(OnePage), (1200.0, 800.0));
    h.settle();
    h.update(|s| roostr_core::nav::open_tab(Route::Object("page-1".into()), true, s));
    h.settle();
    let uids: Vec<u64> = h.state().nav.tabs().iter().map(|t| t.uid).collect();
    let tab = |h: &Harness, i: usize| h.rect(&format!("tab-slot:{}", uids[i])).unwrap();
    let look = |h: &Harness| {
        h.rect("tab-strip-indicator")
            .expect("the active tab's look")
    };
    assert!(
        (look(&h)[0] - tab(&h, 0)[0]).abs() < 0.5,
        "it starts on tab 1"
    );

    // A click on tab 2, as a person picks it.
    let (x, y) = h.rt.find(&format!("tab:{}", uids[1])).expect("tab 2");
    h.rt.pointer_move(x, y);
    h.rt.pointer_down(x, y);
    h.rt.pointer_up(x, y);
    h.frames(2);
    let (from, to) = (tab(&h, 0)[0], tab(&h, 1)[0]);
    let mid = look(&h)[0];
    assert!(
        mid > from + 1. && mid < to - 1.,
        "it is on its way: {mid} between {from} and {to}"
    );
    let home = Route::Home.path();
    assert!(page(&h, &home).is_empty(), "the old page is gone at once");
    let new = page(&h, "/app/object/page-1");
    assert!(
        new.iter().any(|(exiting, o)| !exiting && *o < 0.95),
        "the new page fades in: {new:?}"
    );

    h.frames(60);
    assert!((look(&h)[0] - to).abs() < 0.5, "and lands on tab 2");
    assert!(
        page(&h, "/app/object/page-1")
            .iter()
            .all(|(_, o)| (*o - 1.0).abs() < 0.01),
        "fully shown"
    );
}
