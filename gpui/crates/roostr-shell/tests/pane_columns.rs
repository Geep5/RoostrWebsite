//! The right pane on a desktop: two columns beside the page, Properties then
//! Chat, each expanded or collapsed and each its own width. A collapsed
//! column (beside an expanded one in a narrow window) is a strip with its
//! name that expands it again; the edge drawer tab folds both away and
//! brings both back; a column's resizer drags its width 1:1; and the
//! columns' state and widths outlive the app (a new app on the same UI-state
//! file opens them as they were left). The columns' own headers (collapse
//! buttons, folded chips) belong to the discussion drawer the app draws.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;

use roostr_core::nav::{Route, navigate};
use roostr_core::pane::PaneTab;
use serde_json::Value as Json;

use common::{Harness, OnePage, pane_pages};

/// How a column shows: "col" (expanded, its body drawn), "strip", or "none".
fn how(h: &Harness, tab: &str) -> &'static str {
    let col = h.has(&format!("test-pane-{}", tab.to_lowercase()));
    let strip = h.has(&format!("pane-strip-{tab}"));
    match (col, strip) {
        (true, false) => "col",
        (false, true) => "strip",
        (false, false) => "none",
        (true, true) => panic!("{tab} shows as both a column and a strip"),
    }
}

/// (Chat, Properties), each "col", "strip" or "none".
fn columns(h: &Harness) -> (&'static str, &'static str) {
    (how(h, "Chat"), how(h, "Properties"))
}

/// A desktop shell on `page-1` in a `width` × 900 window, over `ui`.
fn open(width: f32, ui: Json) -> Harness {
    let mut h = Harness::build(Arc::new(OnePage), (width, 900.), ui, pane_pages());
    h.update(|s| navigate(Route::Object("page-1".into()), s));
    h
}

/// Press the edge tab, then move off it before its dwell runs out.
fn tap_edge_tab(h: &mut Harness) {
    let (x, y) = h.rt.find("pane-drawer").expect("the edge tab");
    h.rt.pointer_move(x, y);
    h.rt.pointer_down(x, y);
    h.frame();
    h.rt.pointer_up(x, y);
    h.rt.pointer_move(600., 450.);
    h.settle();
}

#[test]
fn chat_and_properties_are_columns_that_fold_strip_resize_and_persist() {
    let mut h = open(1600., serde_json::json!({}));
    // Both columns by default, Properties then Chat, right of the page.
    assert_eq!(columns(&h), ("col", "col"));
    let chat = h.rect("pane-col-Chat").unwrap();
    let props = h.rect("pane-col-Properties").unwrap();
    let main = h.rect("main-col").unwrap();
    assert!(
        props[0] + props[2] <= chat[0] + 0.5,
        "Properties sits left of Chat"
    );
    assert!(
        main[0] + main[2] <= props[0] + 0.5,
        "the page sits left of the pane"
    );

    // The edge tab folds both away, and brings both back.
    tap_edge_tab(&mut h);
    assert_eq!(columns(&h), ("none", "none"));
    tap_edge_tab(&mut h);
    assert_eq!(columns(&h), ("col", "col"));

    // One collapsed from the page (the drawer's collapse button): the other stays.
    h.update(|s| s.pane.set_open(PaneTab::Chat, false, &mut s.ui));
    assert_eq!(columns(&h), ("none", "col"));
    h.update(|s| s.pane.set_open(PaneTab::Chat, true, &mut s.ui));
    assert_eq!(columns(&h), ("col", "col"));

    // The Chat column's resizer: dragged 100 pt left, the column is 100 pt wider.
    let before = h.rect("pane-col-Chat").unwrap()[2];
    h.drag("pane-resize-Chat", -100., 0., 4);
    let after = h.rect("pane-col-Chat").unwrap()[2];
    assert!(
        (after - before - 100.).abs() < 1.,
        "dragged 100 pt: {before} -> {after}"
    );
    let chat_width = h.state().pane.chat_width;

    // A window too narrow for both (the page keeps its 420 pt): Chat, the
    // column expanded last, and a Properties strip; nothing overflows.
    h.resize(1100., 900.);
    assert_eq!(columns(&h), ("col", "strip"));
    let desk = h.rect("desk").unwrap();
    let strip = h.rect("pane-strip-Properties").unwrap();
    assert!(
        strip[0] + strip[2] <= desk[0] + desk[2] + 0.5,
        "the strip is inside the window"
    );
    // The strip brings Properties in, Chat giving way to its strip.
    h.click("pane-strip-Properties");
    assert_eq!(columns(&h), ("strip", "col"));
    // With the room back, both show again.
    h.resize(1600., 900.);
    assert_eq!(columns(&h), ("col", "col"));
    // Leave Chat expanded and Properties collapsed.
    h.update(|s| s.pane.set_open(PaneTab::Properties, false, &mut s.ui));
    assert_eq!(columns(&h), ("col", "none"));

    // A new app on the same UI-state file opens the columns as they were left.
    let ui: Json = serde_json::from_slice(&std::fs::read(&h.ui_path).unwrap()).unwrap();
    let h = open(1600., ui);
    assert_eq!(columns(&h), ("col", "none"));
    assert_eq!(
        h.state().pane.chat_width,
        chat_width,
        "the Chat column's width"
    );
}
