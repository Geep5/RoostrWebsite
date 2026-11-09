//! A pane column sliding open is the renderer's work: the column's width
//! springs over several frames while the view (and with it the nav and the
//! page) is built once for the change, not once a frame. Field report on
//! GPUI: opening Properties again was laggy because every frame of the slide
//! laid out the whole nav again.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use roostr_core::nav::{Route, navigate};
use roostr_core::pane::PaneTab;
use roostr_shell::Pages;

use common::{Harness, OnePage, pane_pages};

/// How many times the page was drawn (each view run draws it once).
static PAGE_DRAWS: AtomicUsize = AtomicUsize::new(0);

fn counting_pages() -> Pages {
    Pages {
        page: |_, _, _, _| {
            PAGE_DRAWS.fetch_add(1, Ordering::Relaxed);
            rune::el().key("counted-page")
        },
        ..pane_pages()
    }
}

/// The Properties column's drawn width this frame.
fn props_width(h: &Harness) -> f32 {
    h.rt.presented("pane-col-Properties")
        .map_or(0., |(rect, _)| rect[2])
}

#[test]
fn a_pane_column_slides_open_without_rebuilding_the_view_each_frame() {
    let mut h = Harness::with_pages(Arc::new(OnePage), (1600., 800.), counting_pages());
    h.update(|s| navigate(Route::Object("page-1".into()), s));
    h.update(|s| s.pane.set_open(PaneTab::Properties, false, &mut s.ui));
    assert_eq!(props_width(&h), 0., "Properties is collapsed");

    let before = PAGE_DRAWS.load(Ordering::Relaxed);
    h.rt.update(|s, _| s.pane.set_open(PaneTab::Properties, true, &mut s.ui));
    let mut widths = Vec::new();
    for _ in 0..30 {
        h.rt.frame(1.0 / 60.0);
        widths.push(props_width(&h));
    }
    let mut steps = widths.clone();
    steps.dedup();
    assert!(
        steps.len() >= 4,
        "the column slid open over several frames: {widths:?}"
    );
    assert!(
        widths.last().is_some_and(|w| *w > 200.),
        "and ended open: {widths:?}"
    );
    let draws = PAGE_DRAWS.load(Ordering::Relaxed) - before;
    assert!(
        draws <= 3,
        "the view was built {draws} times over the slide's {} frames",
        widths.len()
    );
}
