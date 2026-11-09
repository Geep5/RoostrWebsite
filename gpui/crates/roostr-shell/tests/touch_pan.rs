//! A pan keeps to one axis, by touch, wheel or trackpad. Field reports:
//! scrolling down a query also panned its table sideways - on the iPhone,
//! then on the web with a wheel and a trackpad. A swipe or a wheel down
//! scrolls the page and leaves the table's horizontal strip, a swipe
//! sideways scrolls the strip and leaves the page, a trackpad swipe down
//! that drifts sideways keeps to the page, and a surface that pans in 2D
//! (the graph) takes the whole path of its pans. The phone shell's pages
//! (tables under a page scroll, the graph) rely on all of it.
#![cfg(not(target_family = "wasm"))]

use rune::{
    Cx, Drag, El, Env, GesturePhase, Mods, Phase, Runtime, ScrollSpec, TouchPhase, WheelEvent, col,
    el, row,
};

/// A page that scrolls vertically: a horizontal strip (the table) at
/// y 0-500, then a free-panning canvas (the graph) at y 500-700.
#[derive(Default)]
struct Page {
    /// The canvas's pans: wheel deltas and touch drags, summed.
    panned: [f32; 2],
    /// The drag travel already summed.
    dragged: [f32; 2],
}

fn view(_: &Page, _: &mut Cx<Page>) -> El {
    col().size(600., 800.).child(
        col()
            .key("page")
            .w_full()
            .h(800.)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(el().key("page-top").w_full().h(0.))
            .child(
                row()
                    .key("strip")
                    .w_full()
                    .h(500.)
                    .scroll_x()
                    .child(row().key("strip-body").w(4000.).h(500.)),
            )
            .child(
                el().key("canvas")
                    .w_full()
                    .h(200.)
                    .on_wheel(|s: &mut Page, e: WheelEvent, _| {
                        s.panned[0] += e.dx;
                        s.panned[1] += e.dy;
                        true
                    })
                    .on_drag(|s: &mut Page, d: &mut Drag, _| {
                        if d.phase == Phase::Down {
                            s.dragged = [0., 0.];
                        }
                        s.panned[0] += d.dx - s.dragged[0];
                        s.panned[1] += d.dy - s.dragged[1];
                        s.dragged = [d.dx, d.dy];
                    }),
            )
            .child(el().w_full().h(8000.)),
    )
}

fn open() -> Runtime<Page> {
    let mut rt = Runtime::new(Page::default(), view, (600., 800.), Env::new(true, 2.));
    rt.frame(0.);
    rt
}

fn rect(rt: &Runtime<Page>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .map(|e| e.rect)
        .unwrap_or_else(|| panic!("no {key}"))
}

/// How far the page and the strip have scrolled (points down, right).
fn offsets(rt: &Runtime<Page>) -> (f32, f32) {
    (-rect(rt, "page-top")[1], -rect(rt, "strip-body")[0])
}

/// Run frames until the motion is done.
fn rest(rt: &mut Runtime<Page>) {
    for _ in 0..240 {
        rt.frame(1. / 60.);
    }
}

/// A finger from `from` to `to` in even steps, stopping before it lifts
/// (no fling).
fn swipe(rt: &mut Runtime<Page>, from: [f32; 2], to: [f32; 2]) {
    const STEPS: usize = 12;
    rt.touch(1, TouchPhase::Started, from[0], from[1], 0.);
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let (x, y) = (
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
        );
        rt.touch(1, TouchPhase::Moved, x, y, step as f64 * 0.008);
        rt.frame(1. / 120.);
    }
    rt.touch(
        1,
        TouchPhase::Ended,
        to[0],
        to[1],
        STEPS as f64 * 0.008 + 0.2,
    );
    rest(rt);
}

#[test]
fn a_swipe_down_over_the_strip_scrolls_only_the_page() {
    let mut rt = open();
    // Up the screen and a little right: a mostly vertical swipe.
    swipe(&mut rt, [300., 300.], [330., 150.]);
    let (page, strip) = offsets(&rt);
    assert_eq!(strip, 0., "the strip stays put");
    assert!(page > 100., "the page scrolls down: {page}");
}

#[test]
fn a_swipe_sideways_over_the_strip_scrolls_only_the_strip() {
    let mut rt = open();
    swipe(&mut rt, [300., 250.], [100., 230.]);
    let (page, strip) = offsets(&rt);
    assert_eq!(page, 0., "the page stays put");
    assert!(strip > 150., "the strip scrolls sideways: {strip}");
}

#[test]
fn a_vertical_wheel_over_the_strip_scrolls_only_the_page() {
    let mut rt = open();
    rt.pointer_move(300., 250.);
    rt.wheel_event(WheelEvent::new(0., -60.).at(300., 250.));
    rt.frame(1. / 60.);
    rt.wheel_event(WheelEvent::new(0., -100.).at(300., 250.).precise(true));
    rest(&mut rt);
    let (page, strip) = offsets(&rt);
    assert_eq!(strip, 0., "the strip stays put");
    assert!(page > 100., "the page scrolls down: {page}");
}

#[test]
fn shift_and_the_wheel_scroll_the_strip() {
    let mut rt = open();
    rt.pointer_move(300., 250.);
    let shift = Mods {
        shift: true,
        ..Mods::default()
    };
    // Shift turns a mouse wheel sideways (macOS hands it over turned).
    let wheel = if cfg!(target_os = "macos") {
        WheelEvent::new(-60., 0.)
    } else {
        WheelEvent::new(0., -60.)
    };
    rt.wheel_event(wheel.at(300., 250.).mods(shift));
    rest(&mut rt);
    let (page, strip) = offsets(&rt);
    assert_eq!(page, 0., "the page stays put");
    assert!(strip > 0., "the strip scrolls sideways: {strip}");
}

/// A trackpad gesture: `steps` precise deltas between its phases.
fn trackpad(rt: &mut Runtime<Page>, at: [f32; 2], step: [f32; 2], steps: usize) {
    rt.pointer_move(at[0], at[1]);
    for i in 0..steps {
        let phase = match i {
            0 => GesturePhase::Began,
            _ => GesturePhase::Moved,
        };
        rt.wheel_event(
            WheelEvent::new(step[0], step[1])
                .at(at[0], at[1])
                .precise(true)
                .phase(phase),
        );
        rt.frame(1. / 60.);
    }
    rt.wheel_event(
        WheelEvent::new(0., 0.)
            .at(at[0], at[1])
            .precise(true)
            .phase(GesturePhase::Ended),
    );
    rest(rt);
}

#[test]
fn a_trackpad_swipe_down_that_drifts_sideways_scrolls_only_the_page() {
    let mut rt = open();
    // Down, a point right for every three or four down.
    trackpad(&mut rt, [300., 250.], [-2., -7.], 30);
    let (page, strip) = offsets(&rt);
    assert_eq!(strip, 0., "the strip stays put");
    assert_eq!(page, 210., "the page takes all of it");
}

#[test]
fn a_trackpad_swipe_sideways_scrolls_only_the_strip() {
    let mut rt = open();
    trackpad(&mut rt, [300., 250.], [-7., 2.], 30);
    let (page, strip) = offsets(&rt);
    assert_eq!(page, 0., "the page stays put");
    assert_eq!(strip, 210.);
}

#[test]
fn a_wheel_over_the_canvas_is_not_railed() {
    let mut rt = open();
    trackpad(&mut rt, [300., 600.], [-2., -7.], 10);
    assert_eq!(rt.state.panned, [-20., -70.], "the gesture's whole path");
    assert_eq!(offsets(&rt), (0., 0.));
}

#[test]
fn a_free_canvas_pans_on_both_axes() {
    let mut rt = open();
    swipe(&mut rt, [100., 580.], [220., 480.]);
    assert_eq!(rt.state.panned, [120., -100.], "the finger's whole path");
    assert_eq!(offsets(&rt), (0., 0.));
}
