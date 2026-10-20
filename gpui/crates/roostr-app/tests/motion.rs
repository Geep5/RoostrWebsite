//! `roostr_core::motion`'s keyed helpers in a real window: a value shows its
//! target on first render and eases after a change, reduced motion snaps it,
//! and a closed menu leaves the very next frame (nothing holds input or
//! focus while it fades), reopening from its hidden start.
#![cfg(not(target_family = "wasm"))]

use std::time::Duration;

use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div, px,
};
use roostr_core::motion;

struct Probe {
    target: f32,
    menu_open: bool,
    /// The presented value and the menu's opacity, from the last render.
    value: f32,
    menu_opacity: Option<f32>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.value = motion::animate("probe-value", self.target, motion::CONTROL, window, cx);
        self.menu_opacity = None;
        let menu = self.menu_open.then(|| {
            let frame = motion::appear_frame("probe-menu", -4., window, cx);
            self.menu_opacity = Some(frame.target().alpha());
            frame.apply(div().debug_selector(|| "probe-menu".into()).size(px(40.)))
        });
        div().size_full().children(menu)
    }
}

fn frame_after(cx: &mut VisualTestContext, view: &Entity<Probe>, elapsed: Duration) {
    cx.executor().advance_clock(elapsed);
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

fn open(cx: &mut TestAppContext) -> (Entity<Probe>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|_, _| Probe {
        target: 2.,
        menu_open: false,
        value: f32::NAN,
        menu_opacity: None,
    })
}

#[gpui::test]
fn values_ease_after_a_change_and_snap_under_reduced_motion(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |p, _| p.value),
        2.,
        "no animation on mount"
    );

    view.update(cx, |p, cx| {
        p.target = 16.;
        cx.notify();
    });
    cx.run_until_parked();
    frame_after(cx, &view, Duration::from_millis(75));
    let mid = view.read_with(cx, |p, _| p.value);
    assert!(mid > 2. && mid < 16., "eases: {mid}");
    frame_after(cx, &view, motion::CONTROL.duration);
    assert_eq!(view.read_with(cx, |p, _| p.value), 16.);

    cx.update(|_, cx| cx.set_reduce_motion(true));
    view.update(cx, |p, cx| {
        p.target = 2.;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |p, _| p.value),
        2.,
        "reduced motion snaps"
    );
}

#[gpui::test]
fn a_closed_menu_leaves_at_once_and_reopens_from_hidden(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let set_open = |open: bool, cx: &mut VisualTestContext| {
        view.update(cx, |p, cx| {
            p.menu_open = open;
            cx.notify();
        });
        cx.run_until_parked();
    };

    set_open(true, cx);
    assert_eq!(view.read_with(cx, |p, _| p.menu_opacity), Some(0.));
    assert!(
        cx.debug_bounds("probe-menu").is_some(),
        "takes input from its first frame"
    );
    frame_after(cx, &view, motion::FADE.duration);
    assert_eq!(view.read_with(cx, |p, _| p.menu_opacity), Some(1.));

    set_open(false, cx);
    assert!(
        cx.debug_bounds("probe-menu").is_none(),
        "gone the next frame"
    );

    set_open(true, cx);
    assert_eq!(
        view.read_with(cx, |p, _| p.menu_opacity),
        Some(0.),
        "a reopened menu appears again"
    );

    cx.update(|_, cx| cx.set_reduce_motion(true));
    set_open(false, cx);
    set_open(true, cx);
    assert_eq!(
        view.read_with(cx, |p, _| p.menu_opacity),
        Some(1.),
        "reduced motion snaps"
    );
}
