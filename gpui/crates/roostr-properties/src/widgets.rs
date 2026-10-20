//! Small private building blocks shared by this crate's views: popover
//! surfaces anchored under their trigger, the click-away backdrop, the text
//! field skins the web app uses, and a few text helpers.

use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
pub(crate) use roostr_core::ui::{menu_shadow, modal_shadow, tooltip as tip};
use roostr_core::{motion, theme};

/// `0 12px 32px rgba(0,0,0,.45)`: the popovers' shadow.
pub(crate) fn pop_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.45),
        offset: point(px(0.), px(12.)),
        blur_radius: px(32.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// Hang `content` below the element it is placed in (which must be
/// `.relative()`), `gap` under its bottom edge, kept on screen. It drops in
/// as it opens (`id` keys that, unique among the view's open popovers) and
/// takes input from its first frame.
pub(crate) fn below(
    id: impl Into<ElementId>,
    gap: f32,
    content: impl IntoElement,
) -> impl IntoElement {
    div().absolute().left_0().top_full().child(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(Appear {
                    id: id.into(),
                    gap,
                    content: content.into_any_element(),
                }),
        )
        .with_priority(2),
    )
}

/// [`below`]'s panel, entering with [`motion::appear`].
#[derive(IntoElement)]
struct Appear {
    id: ElementId,
    gap: f32,
    content: AnyElement,
}

impl RenderOnce for Appear {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        motion::appear_frame(self.id, -4., window, cx)
            .apply(div().mt(px(self.gap)).child(self.content))
    }
}

/// A row the user just added entering its list: opacity 0 → 1 while it rises
/// 6px into place over [`motion::CONTROL`]. Removal is immediate.
pub(crate) fn enter_frame(
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
) -> motion::Frame {
    let enter = motion::Motion::new(motion::Target::default())
        .initial(motion::Target::default().y(6.).opacity(0.))
        .transition(motion::CONTROL.transition());
    motion::use_motion(id, window, cx, motion::MotionState::default)
        .update(cx, |state, cx| state.frame(enter, window, cx))
}

/// An invisible full-window layer under an open popover: a press anywhere
/// outside the popover closes it (the web app's `.backdrop` / window
/// pointerdown handlers).
pub(crate) fn backdrop(
    id: impl Into<ElementId>,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_close = Rc::new(on_close);
    deferred(
        anchored()
            .position(point(px(0.), px(0.)))
            .position_mode(AnchoredPositionMode::Window)
            .child(
                div()
                    .id(id.into())
                    .w(px(10000.))
                    .h(px(10000.))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, {
                        let on_close = on_close.clone();
                        move |_, window, cx| {
                            cx.stop_propagation();
                            on_close(window, cx)
                        }
                    })
                    .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                        cx.stop_propagation();
                        on_close(window, cx)
                    }),
            ),
    )
    .with_priority(1)
}

/// The look of a single-line text box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldSkin {
    /// Property value editors: `--bg`, 1px border, radius 6, 5px 8px, 13px.
    Value,
    /// Anytype's filter field: filled `hl-light` (focus `hl-med`), borderless, radius 10, 8px 12px.
    Filter,
    /// Menu search boxes: `--bg`, 1px border (accent on focus), radius 6, 6px 8px.
    Search,
    /// TypeSuggest: `--bg`, 1px border (accent on focus), radius 8, 6px 10px.
    Suggest,
    /// The emoji picker's search: `--bg`, border, radius 7, 5px 9px.
    Emoji,
    /// Repeat editor fields: white 5% fill, 1px border, radius 6, 3px 6px, 12.5px.
    Compact,
}

/// A styled single-line text box around `state`.
pub(crate) fn field(state: &Entity<InputState>, skin: FieldSkin, window: &Window, cx: &App) -> Div {
    let focused = state.focus_handle(cx).is_focused(window);
    let (text, line) = match skin {
        FieldSkin::Compact => (px(12.5), px(17.)),
        _ => (px(13.), px(17.)),
    };
    let input = Input::new(state)
        .appearance(false)
        .text_size(text)
        .p_0()
        .h(line)
        .w_full();
    let base = div()
        .flex()
        .items_center()
        .min_w_0()
        .text_color(theme::fg())
        .text_size(text)
        .child(input);
    match skin {
        FieldSkin::Value => base
            .bg(theme::bg())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::sm())
            .px(px(8.))
            .py(px(5.)),
        FieldSkin::Filter => base
            .bg(if focused {
                theme::hl_med()
            } else {
                theme::hl_light()
            })
            .rounded(theme::radius::menu())
            .px(px(12.))
            .py(px(8.)),
        FieldSkin::Search => base
            .bg(theme::bg())
            .border_1()
            .border_color(if focused {
                theme::accent()
            } else {
                theme::border()
            })
            .rounded(theme::radius::sm())
            .px(px(8.))
            .py(px(6.)),
        FieldSkin::Suggest => base
            .bg(theme::bg())
            .border_1()
            .border_color(if focused {
                theme::accent()
            } else {
                theme::border()
            })
            .rounded(theme::radius::md())
            .px(px(10.))
            .py(px(6.)),
        FieldSkin::Emoji => base
            .bg(theme::bg())
            .border_1()
            .border_color(if focused {
                theme::accent()
            } else {
                theme::border()
            })
            .rounded(px(7.))
            .px(px(9.))
            .py(px(5.)),
        FieldSkin::Compact => base
            .bg(hsla(0., 0., 1., 0.05))
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::sm())
            .px(px(6.))
            .py(px(3.)),
    }
}

/// A styled multi-line text box: the value editors' textarea.
pub(crate) fn text_area(
    state: &Entity<TextareaState>,
    window: &Window,
    cx: &App,
    bg: Hsla,
    radius: Pixels,
    pad: (f32, f32),
) -> Div {
    let focused = state.focus_handle(cx).is_focused(window);
    div()
        .w_full()
        .min_w_0()
        .bg(bg)
        .border_1()
        .border_color(if focused {
            theme::accent()
        } else {
            theme::border()
        })
        .rounded(radius)
        .py(px(pad.0))
        .px(px(pad.1))
        .text_size(px(13.))
        .text_color(theme::fg())
        .child(
            Textarea::new(state)
                .appearance(false)
                .text_size(px(13.))
                .p_0()
                .w_full(),
        )
}

/// An empty row value: one faint dash; the hint replaces it while the row
/// (`group`) is hovered.
pub(crate) fn placeholder(group: SharedString, hint: impl Into<SharedString>) -> Div {
    // Visibility, not display: a hover style may not change layout between
    // prepaint and paint.
    div()
        .relative()
        .text_color(theme::muted())
        .opacity(0.6)
        .whitespace_nowrap()
        .child(
            div()
                .group_hover(group.clone(), |s| s.invisible())
                .child("—"),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .invisible()
                .group_hover(group, |s| s.visible())
                .child(hint.into()),
        )
}

/// A plain outline action button (`.act`): bg, 1px border, radius 7, 5px 12px, 12px.
pub(crate) fn act_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id.into())
        .flex_none()
        .bg(theme::bg())
        .border_1()
        .border_color(theme::border())
        .rounded(px(7.))
        .px(px(12.))
        .py(px(5.))
        .text_size(px(12.))
        .text_color(theme::fg())
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.border_color(theme::accent()))
        })
        .child(label)
}

/// The first `n` characters of `s` (char-safe `slice(0, n)`).
pub(crate) fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
