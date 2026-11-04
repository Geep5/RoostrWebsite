//! roostr: the axis a touch pan locks to (`gpui::touch_pan_axis`).

use gpui::{Axis, point, px, touch_pan_axis};

#[test]
fn a_mostly_vertical_pan_is_vertical() {
    // A swipe down that drifts right, and one up that drifts left.
    assert_eq!(touch_pan_axis(point(px(3.), px(9.))), Axis::Vertical);
    assert_eq!(touch_pan_axis(point(px(-6.), px(-8.))), Axis::Vertical);
    assert_eq!(touch_pan_axis(point(px(0.), px(9.))), Axis::Vertical);
}

#[test]
fn a_mostly_horizontal_pan_is_horizontal() {
    assert_eq!(touch_pan_axis(point(px(9.), px(3.))), Axis::Horizontal);
    assert_eq!(touch_pan_axis(point(px(-8.), px(6.))), Axis::Horizontal);
    assert_eq!(touch_pan_axis(point(px(9.), px(0.))), Axis::Horizontal);
}

#[test]
fn an_exact_diagonal_is_vertical() {
    // A page scrolls vertically; the tie goes to it.
    assert_eq!(touch_pan_axis(point(px(6.), px(6.))), Axis::Vertical);
    assert_eq!(touch_pan_axis(point(px(-6.), px(6.))), Axis::Vertical);
}
