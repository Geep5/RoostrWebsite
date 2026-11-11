//! Small private building blocks shared by this crate's views: popover
//! surfaces anchored under their trigger, the click-away backdrop, the text
//! field skins the web app uses, a few text helpers, and the plumbing every
//! view uses to reach its state (lenses, deferred opens).

use std::rc::Rc;

use roostr_core::State;
use roostr_ui::{
    Alert, Button, Input, KitSize, Placement, Textarea, Theme, motion, radius, shadow, ty,
};
use rune::{Color, Effect, El, Shadow, Spring, SpringPair, Transition, Vec2, col, el, row, text};

pub(crate) use roostr_ui::tooltip as tip;

/// Borrows one piece of state out of [`State`] (a host's menu state, an
/// editor in this crate's slot). Handlers call it; views never do.
pub(crate) type Lens<T> = Rc<dyn for<'a> Fn(&'a mut State) -> &'a mut T>;

/// Makes a [`Lens`] from a closure (ties its returned borrow to the state).
pub(crate) fn lens<T: ?Sized + 'static>(
    f: impl for<'a> Fn(&'a mut State) -> &'a mut T + 'static,
) -> Rc<dyn for<'a> Fn(&'a mut State) -> &'a mut T> {
    Rc::new(f)
}

/// Runs `f` with the state at the next poll: how a view (which only reads
/// the state) opens its own per-object state on first draw, as GPUI's
/// `new` did. `f` must be idempotent: a view may ask more than once before
/// the poll runs it.
pub(crate) fn later(s: &State, f: impl FnOnce(&mut State) + 'static) {
    s.rt.spawn(async move |cx| cx.update(f)).detach();
}

/// `0 12px 32px rgba(0,0,0,.45)`: the popovers' shadow.
pub(crate) fn pop_shadow() -> Shadow {
    shadow::gpui(12.0, 32.0, 0.0, 0.45)
}

/// The menus' shadow (`roostr_core::ui::menu_shadow`).
pub(crate) fn menu_shadow() -> Shadow {
    shadow::menu()
}

/// Hangs `content` below the element keyed `anchor`, `gap` under its bottom
/// edge, start-aligned and kept on screen; it drops in as it opens
/// ([`motion::appear`]) and takes input from its first frame.
pub(crate) fn below(layer: &mut Vec<El>, anchor: &str, gap: f32, content: El) {
    layer.push(roostr_ui::overlay::anchored(
        content,
        anchor,
        Placement::BottomStart,
        gap,
    ));
}

/// An invisible full-window layer under an open popover: a press anywhere
/// outside the popover closes it (the web app's `.backdrop`). Push it before
/// the popover so the popover draws above it.
pub(crate) fn backdrop(layer: &mut Vec<El>, key: &str, close: impl Fn(&mut State) + 'static) {
    layer.push(roostr_ui::overlay::scrim(key.to_string(), close));
}

/// A row the user just added entering its list: opacity 0 → 1 while it rises
/// 6px into place over [`motion::CONTROL`]. Removal is immediate. Rune runs
/// it when the element first appears after the first frame (keep it keyed).
pub(crate) fn enter(e: El) -> El {
    e.transition(Transition {
        insertion: Effect {
            offset: Vec2::new(0.0, 6.0),
            ..Effect::opacity(0.0)
        },
        removal: Effect::IDENTITY,
        springs: SpringPair::new(motion::CONTROL.spring(), Spring::NONE),
        ..Transition::identity()
    })
}

/// The look of a single-line text box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldSkin {
    /// Property value editors: `--bg`, 1px border, radius 6, 5px 8px, 13px.
    Value,
    /// Anytype's filter field: filled `hl-light` (focus `hl-med`), borderless, radius 10, 8px 12px.
    Filter,
    /// The emoji picker's search: `--bg`, border (accent when focused), radius 7, 5px 9px.
    Emoji,
    /// Repeat editor fields: white 5% fill, 1px border, radius 6, 3px 6px, 12.5px.
    Compact,
}

/// A styled single-line text box around `input` (bound by the caller with
/// `on_edit`, `on_change`, `on_submit`, `on_key`, `placeholder`). `focused`
/// is the field state's `is_focused()`.
pub(crate) fn field(t: &Theme, input: Input, focused: bool, skin: FieldSkin) -> El {
    let size = match skin {
        FieldSkin::Compact => 12.5,
        _ => 13.0,
    };
    let input = input
        .appearance(false)
        .text_size(size)
        .line_height(17.0)
        .text_color(t.fg)
        .px(0.0)
        .py(0.0)
        .h(17.0)
        .build(t)
        .grow(1.0)
        .min_w(0.0);
    // The whole field width is its fill and takes the press, not just the text.
    let base = row().w_full().items_center().min_w(0.0).child(input);
    match skin {
        FieldSkin::Value => base
            .fill(t.bg)
            .border(1.0, t.border)
            .radius(radius::SM)
            .px(8.0)
            .py(5.0),
        FieldSkin::Filter => base
            .fill(if focused { t.hl_med } else { t.hl_light })
            .radius(radius::MENU)
            .px(12.0)
            .py(8.0),
        FieldSkin::Emoji => base
            .fill(t.bg)
            .border(1.0, if focused { t.accent } else { t.border })
            .radius(7.0)
            .px(9.0)
            .py(5.0),
        FieldSkin::Compact => base
            .fill(Color::WHITE.alpha(0.05))
            .border(1.0, t.border)
            .radius(radius::SM)
            .px(6.0)
            .py(3.0),
    }
}

/// A styled multi-line text box: the value editors' textarea. `focused` is
/// the area state's `is_focused()`; `pad` is (vertical, horizontal).
pub(crate) fn text_area(
    t: &Theme,
    area: Textarea,
    focused: bool,
    bg: Color,
    radius: f32,
    pad: (f32, f32),
) -> El {
    col()
        .w_full()
        .min_w(0.0)
        .fill(bg)
        .border(1.0, if focused { t.accent } else { t.border })
        .radius(radius)
        .py(pad.0)
        .px(pad.1)
        .child(
            area.appearance(false)
                .text_size(13.0)
                .text_color(t.fg)
                .px(0.0)
                .py(0.0)
                .build(t),
        )
}

/// An empty row value: one faint dash; the hint replaces it while the row
/// around it (an ancestor with `.hover_group()`) is hovered.
pub(crate) fn placeholder(t: &Theme, hint: impl Into<String>) -> El {
    let style = ty::ui(13.0);
    el().opacity(0.6)
        .child(text("—").font(style).color(t.muted).group_opacity(1.0, 0.0))
        .child(
            text(hint.into())
                .font(style)
                .color(t.muted)
                .absolute(0.0, 0.0)
                .group_opacity(0.0, 1.0),
        )
}

/// A list's one-line empty note: Roostr's quiet left-aligned muted line at
/// `size`; callers set its padding.
pub(crate) fn empty_note(t: &Theme, note: impl Into<String>, size: f32) -> El {
    text(note.into()).font(ty::ui(size)).color(t.muted).wrap()
}

/// A plain outline action button (`.act`): bg, 1px border, radius 7, 5px
/// 12px, 12px label; kit hover fill; `disabled` greys it and blocks presses.
/// The caller adds `on_press` and builds it.
pub(crate) fn act_button(
    t: &Theme,
    key: impl Into<String>,
    label: impl Into<String>,
    disabled: bool,
) -> Button {
    let label: String = label.into();
    roostr_ui::disabled(
        Button::new(key)
            .outline()
            .size(KitSize::Small)
            .accessibility_label(label.clone())
            .label(label)
            .radius(7.0)
            .h_auto()
            .px(12.0)
            .py(5.0)
            .bg(t.bg)
            .border_color(t.border)
            .text_color(t.fg)
            .text_size(12.0),
        disabled,
    )
}

/// A muted one-line note: `size`pt in the muted colour.
pub(crate) fn note(t: &Theme, line: impl Into<String>, size: f32) -> El {
    text(line.into()).font(ty::ui(size)).color(t.muted).wrap()
}

/// An error line under a control: the kit destructive alert, compact.
pub(crate) fn error_alert(t: &Theme, key: impl Into<String>, line: impl Into<String>) -> El {
    Alert::error(key, line).size(KitSize::XSmall).build(t)
}

/// A 1px horizontal rule in the border colour.
pub(crate) fn rule(t: &Theme) -> El {
    el().h(1.0).w_full().fill(t.border)
}

/// Recolours `child` (text or icon) to `color` while the nearest ancestor
/// with `.hover_group()` is hovered.
pub(crate) fn hover_tint(child: El, color: Color) -> El {
    child.group_color(color)
}

/// The first `n` characters of `s` (char-safe `slice(0, n)`).
pub(crate) fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// roostr-core's glyph (what `type_glyph` and friends return) as the
/// roostr-ui glyph that draws it (same names).
pub(crate) fn ui_glyph(g: roostr_core::Glyph) -> roostr_ui::Glyph {
    roostr_ui::Glyph::from_name(g.name()).expect("roostr-ui has every Roostr glyph")
}

/// An object/space icon in a `size` square ([`roostr_ui::emoji_or_image_icon`])
/// with the window's image cache; an image icon stays an empty square where
/// there is none (headless tests).
pub(crate) fn icon_el(s: &State, icon: &str, size: f32) -> El {
    match s.get::<roostr_ui::Images>() {
        Some(images) => roostr_ui::emoji_or_image_icon(icon, size, images),
        None if roostr_ui::is_image_icon(icon) => el().size(size, size),
        None => row()
            .size(size, size)
            .center()
            .child(roostr_ui::emoji_text(icon, ty::ui(size * 0.9).lh(size))),
    }
}

/// A row icon as text in the row's `style`, or a `size` square image
/// ([`roostr_ui::icon_text`]) with the window's image cache.
pub(crate) fn icon_text_el(s: &State, icon: &str, size: f32, style: rune::TextStyle) -> El {
    match s.get::<roostr_ui::Images>() {
        Some(images) => roostr_ui::icon_text(icon, size, style, images),
        None if roostr_ui::is_image_icon(icon) => el().size(size, size),
        None => roostr_ui::emoji_text(icon, style),
    }
}
