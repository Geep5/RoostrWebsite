//! A touch pan keeps to one axis (vendor/gpui-pre). Field report (iPhone,
//! build 51): scrolling down a query also panned its table sideways. A pan
//! was already locked to the axis it started on, but a scroller that scrolls
//! on one axis only (the table's horizontal strip, under the page's vertical
//! scroll) turned the other axis into its own, as it does for a mouse wheel.
//! Now a swipe down scrolls the page and leaves the strip, a swipe sideways
//! scrolls the strip and leaves the page, the wheel still turns the strip,
//! and a surface that pans in 2D (the graph) frees its pans from the lock.
#![cfg(not(target_family = "wasm"))]

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Modifiers, ScrollDelta, ScrollHandle, ScrollWheelEvent, TestAppContext, TouchEvent, TouchId,
    TouchPhase, VisualTestContext,
};
use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
    Render, StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

/// A page that scrolls vertically: a horizontal strip (the table) at
/// y 0-500, then a free-panning canvas (the graph) at y 500-700.
struct Page {
    page: ScrollHandle,
    strip: ScrollHandle,
    panned: Rc<Cell<Point<Pixels>>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let panned = self.panned.clone();
        div().size_full().child(
            div()
                .id("page")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.page)
                .child(
                    div()
                        .id("strip")
                        .w_full()
                        .h(px(500.))
                        .overflow_x_scroll()
                        .track_scroll(&self.strip)
                        .child(div().w(px(4000.)).h(px(500.))),
                )
                .child(div().id("canvas").w_full().h(px(200.)).on_scroll_wheel(
                    move |e, window, cx| {
                        window.pan_touch_freely();
                        panned.set(panned.get() + e.delta.pixel_delta(px(20.)));
                        cx.stop_propagation();
                    },
                ))
                .child(div().h(px(8000.))),
        )
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| Page {
        page: ScrollHandle::new(),
        strip: ScrollHandle::new(),
        panned: Rc::default(),
    });
    cx.run_until_parked();
    (view, cx)
}

fn touch(cx: &mut VisualTestContext, id: u64, phase: TouchPhase, at: Point<Pixels>) {
    cx.simulate_event(TouchEvent {
        id: TouchId(id),
        phase,
        position: at,
        ..TouchEvent::default()
    });
}

/// A finger from `from` to `to` in even steps, stopping before it lifts
/// (no fling).
fn swipe(cx: &mut VisualTestContext, id: u64, from: Point<Pixels>, to: Point<Pixels>) {
    touch(cx, id, TouchPhase::Started, from);
    const STEPS: usize = 12;
    for step in 1..=STEPS {
        cx.executor().advance_clock(Duration::from_millis(8));
        let t = step as f32 / STEPS as f32;
        touch(cx, id, TouchPhase::Moved, from + (to - from) * t);
    }
    cx.executor().advance_clock(Duration::from_millis(200));
    touch(cx, id, TouchPhase::Ended, to);
    cx.run_until_parked();
}

fn offsets(view: &Entity<Page>, cx: &mut VisualTestContext) -> (Point<Pixels>, Point<Pixels>) {
    view.read_with(cx, |p, _| (p.page.offset(), p.strip.offset()))
}

#[gpui::test]
fn a_swipe_down_over_the_strip_scrolls_only_the_page(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // Up the screen and a little right: a mostly vertical swipe.
    swipe(cx, 1, point(px(300.), px(300.)), point(px(330.), px(150.)));
    let (page, strip) = offsets(&view, cx);
    assert_eq!(strip, Point::default(), "the strip stays put");
    assert_eq!(page.x, px(0.));
    assert!(page.y < px(-100.), "the page scrolls down: {page:?}");
}

#[gpui::test]
fn a_swipe_sideways_over_the_strip_scrolls_only_the_strip(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    swipe(cx, 1, point(px(300.), px(250.)), point(px(100.), px(230.)));
    let (page, strip) = offsets(&view, cx);
    assert_eq!(page, Point::default(), "the page stays put");
    assert_eq!(strip.y, px(0.));
    assert!(strip.x < px(-150.), "the strip scrolls sideways: {strip:?}");
}

#[gpui::test]
fn a_wheel_still_turns_the_strip(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(300.), px(250.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    let (_, strip) = offsets(&view, cx);
    assert_eq!(strip, point(px(-50.), px(0.)));
}

#[gpui::test]
fn a_free_canvas_pans_on_both_axes(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    swipe(cx, 1, point(px(100.), px(580.)), point(px(220.), px(480.)));
    let panned = view.read_with(cx, |p, _| p.panned.get());
    assert_eq!(
        panned,
        point(px(120.), px(-100.)),
        "the finger's whole path"
    );
    let (page, strip) = offsets(&view, cx);
    assert_eq!((page, strip), (Point::default(), Point::default()));
}
