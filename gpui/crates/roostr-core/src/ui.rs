//! Shared UI primitives, styled per `.specs/roostr/style.md` §5 so every
//! feature crate looks the same: checkboxes (kit `Checkbox`), property icons,
//! tag pills (kit `Tag`), property badges (kit `Tag` in a kit `Button`),
//! popover menus, primary/outline/danger/icon buttons (kit `Button`), section
//! labels, modal frames, tooltips (kit `Tooltip`) and emoji-or-image icons.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip as KitTooltip;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::Glyph;
use crate::objects::is_image_icon;
use crate::options::{TagPaint, badge_colors, tag_colors, unknown_tag_paint};
use crate::theme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

// ------------------------------------------------------------------ shadows

fn shadow(y: f32, blur: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// Menus / popovers: `0 16px 48px rgb(0 0 0/0.5)`.
pub fn menu_shadow() -> Vec<BoxShadow> {
    vec![shadow(16., 48., 0.5)]
}

/// Modals: `0 24px 80px rgb(0 0 0/0.6)`.
pub fn modal_shadow() -> Vec<BoxShadow> {
    vec![shadow(24., 80., 0.6)]
}

/// Tooltips: `0 4px 14px rgb(0 0 0/0.25)`.
pub fn tooltip_shadow() -> Vec<BoxShadow> {
    vec![shadow(4., 14., 0.25)]
}

/// Lifted drag card: `0 10px 28px rgb(0 0 0/0.45)`.
pub fn drag_shadow() -> Vec<BoxShadow> {
    vec![shadow(10., 28., 0.45)]
}

/// CSS `filter: brightness(f)` on a fill (approximation in HSL lightness).
pub fn brighten(color: Hsla, f: f32) -> Hsla {
    let c = color.to_rgb();
    Rgba {
        r: (c.r * f).min(1.),
        g: (c.g * f).min(1.),
        b: (c.b * f).min(1.),
        a: c.a,
    }
    .into()
}

// -------------------------------------------------------------------- icons

/// The kit [`Checkbox`] at Roostr's size: Anytype's 20px box drawn at the
/// kit's largest size (18px), accent (`primary`) fill when checked. Chain
/// `.on_change(|checked: &bool, window, cx| ..)` (controlled: write the value
/// and notify; call `cx.stop_propagation()` there when it sits in a clickable
/// row), `.tooltip("Mark done")`, `.disabled(..)`, or `.with_size(Size::Medium)`
/// (16px) / `.small()` (14px) / `.xsmall()` (12px) for denser rows.
pub fn checkbox(id: impl Into<ElementId>, checked: bool) -> Checkbox {
    Checkbox::new(id).checked(checked).with_size(Size::Large)
}

/// A display-only [`checkbox`]: full colour, no click handler, out of the tab
/// order; presses fall through to the surrounding row.
pub fn checkbox_display(id: impl Into<ElementId>, checked: bool) -> Checkbox {
    checkbox(id, checked).tab_stop(false)
}

/// The line icon for a relation format (`PropIcon` `line`).
pub fn prop_icon(format: &str) -> Glyph {
    match format {
        "checkbox" => Glyph::PropCheckbox,
        "date" => Glyph::PropDate,
        "number" => Glyph::PropNumber,
        "status" | "select" => Glyph::PropStatus,
        "tag" => Glyph::PropTag,
        "url" => Glyph::PropUrl,
        "email" => Glyph::PropEmail,
        "phone" => Glyph::PropPhone,
        "object" => Glyph::PropObject,
        "file" => Glyph::PropFile,
        _ => Glyph::PropText,
    }
}

/// A property row's leading icon (`PropertiesPane` `lineIcon`): well-known
/// keys first, then the format. (Credential secret fields: callers pass `Glyph::PropKey`.)
pub fn prop_line_icon(key: &str, format: &str) -> Glyph {
    match key {
        "served_by" => Glyph::PropComputer,
        "repo_path" => Glyph::PropFolder,
        "chat_color" => Glyph::PropColor,
        "credentials" | "session_cookie" | "key" => Glyph::PropKey,
        "agent" | "account" => Glyph::PropPerson,
        "skills" => Glyph::PropSkill,
        "tools" => Glyph::PropTool,
        "check_first" => Glyph::PropSearch,
        "prompt" => Glyph::PropBrain,
        "model" => Glyph::PropCpu,
        "error" => Glyph::PropWarning,
        "capability" | "service" => Glyph::PropBox,
        "session_host" => Glyph::PropGlobe,
        "checked_at" => Glyph::PropClock,
        _ => prop_icon(format),
    }
}

static DATA_IMAGES: LazyLock<Mutex<HashMap<String, Option<Arc<Image>>>>> =
    LazyLock::new(Default::default);

/// A `data:image/…` URL (base64 or percent-encoded) decoded as a GPUI image; cached by URL.
pub fn data_url_image(uri: &str) -> Option<Arc<Image>> {
    if let Some(hit) = DATA_IMAGES.lock().ok()?.get(uri) {
        return hit.clone();
    }
    let decoded = (|| {
        let (meta, data) = uri.strip_prefix("data:")?.split_once(',')?;
        let format = match meta.split(';').next()? {
            "image/png" => ImageFormat::Png,
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/webp" => ImageFormat::Webp,
            "image/gif" => ImageFormat::Gif,
            "image/svg+xml" => ImageFormat::Svg,
            "image/bmp" => ImageFormat::Bmp,
            "image/tiff" => ImageFormat::Tiff,
            "image/x-icon" | "image/vnd.microsoft.icon" => ImageFormat::Ico,
            _ => return None,
        };
        let bytes = if meta.ends_with(";base64") {
            base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .ok()?
        } else {
            percent_encoding::percent_decode_str(data).collect()
        };
        Some(Arc::new(Image::from_bytes(format, bytes)))
    })();
    if let Ok(mut cache) = DATA_IMAGES.lock() {
        cache.insert(uri.to_string(), decoded.clone());
    }
    decoded
}

/// An object/space icon: an `img()` for a web address or `data:image/` URL
/// (rounded, cover), else the emoji/glyph text at `size`, centered in a
/// `size` square.
pub fn emoji_or_image_icon(icon: &str, size: Pixels) -> AnyElement {
    if is_image_icon(icon) {
        let source: ImageSource = if icon.starts_with("data:") {
            match data_url_image(icon) {
                Some(image) => ImageSource::Image(image),
                None => return div().size(size).flex_none().into_any_element(),
            }
        } else {
            SharedUri::from(icon.to_string()).into()
        };
        return img(source)
            .size(size)
            .flex_none()
            .rounded(theme::radius::xs())
            .object_fit(ObjectFit::Cover)
            .into_any_element();
    }
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(size * 0.9)
        .line_height(size)
        .child(SharedString::from(icon.to_string()))
        .into_any_element()
}

/// A row icon: an emoji/glyph as plain text in the surrounding font size,
/// or a `size` square `img()` for a web address or `data:image/` URL.
pub fn icon_text(icon: &str, size: Pixels) -> AnyElement {
    if is_image_icon(icon) {
        return emoji_or_image_icon(icon, size);
    }
    div()
        .flex_none()
        .child(SharedString::from(icon.to_string()))
        .into_any_element()
}

// ------------------------------------------------------------- pills/badges

/// An option pill (Anytype tagItem): h20, padding 0 6, radius 10, 12px, in
/// the kit `Tag::color` look of its palette colour; unknown colour neutral
/// with primary text.
#[derive(IntoElement)]
pub struct TagPill {
    text: SharedString,
    color: SharedString,
}

impl TagPill {
    pub fn new(text: impl Into<SharedString>, color: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            color: color.into(),
        }
    }
}

impl RenderOnce for TagPill {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let paint = tag_colors(&self.color).unwrap_or_else(|| unknown_tag_paint(theme::fg()));
        tag(paint, px(20.), px(12.)).child(self.text)
    }
}

/// The kit [`Tag`](gpui_kit::component::tag::Tag) as Roostr draws an option:
/// `paint`'s fill, 1px border and text, radius 10, 0 6px padding, one line of
/// `height` and `text` size, ellipsis when squeezed.
pub fn tag(paint: TagPaint, height: Pixels, text: Pixels) -> gpui_kit::component::tag::Tag {
    gpui_kit::component::tag::Tag::custom(paint.bg, paint.fg, paint.border)
        .rounded(px(10.))
        .flex_none()
        .h(height)
        .line_height(height - px(2.))
        .px(px(6.))
        .py_0()
        .text_size(text)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
}

/// A property badge (`ObjectMeta .badge`): a kit [`Button`] painting the
/// badge fill (radius 8, hover brightness 1.18; pointer, keyboard focus and
/// tooltip when clickable) around a kit [`Tag`] holding the content: h26,
/// padding 0 10 0 6, gap 6, 13/500, a leading 16px glyph in the badge's icon
/// color.
#[derive(IntoElement)]
pub struct Badge {
    id: ElementId,
    text: SharedString,
    color: SharedString,
    icon: Glyph,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl Badge {
    /// `color` is a palette name ("" = neutral); `icon` usually
    /// [`crate::options::status_icon`] or a `Badge*` glyph.
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        color: impl Into<SharedString>,
        icon: Glyph,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            color: color.into(),
            icon,
            tooltip: None,
            on_click: None,
        }
    }

    /// The kit tooltip shown on hover.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = badge_colors(&self.color);
        let hover = brighten(colors.bg, 1.18);
        let clickable = self.on_click.is_some();
        let fill = ButtonCustomVariant::new(cx)
            .color(colors.bg)
            .foreground(colors.fg)
            .hover(hover)
            .active(hover)
            .shadow(false);
        Button::new(self.id)
            .custom(fill)
            .rounded(px(8.))
            .accessibility_label(self.text.clone())
            .tab_stop(clickable)
            .flex_none()
            .h(px(26.))
            .p_0()
            .bg(colors.bg)
            .text_color(colors.fg)
            .when(clickable, |b| b.cursor_pointer())
            .when_some(self.tooltip, |b, text| b.tooltip(text))
            .when_some(self.on_click, |b, handler| b.on_click(handler))
            .child(
                Tag::custom(transparent_black(), colors.fg, transparent_black())
                    .rounded(px(8.))
                    .border_0()
                    .h_full()
                    .gap(px(6.))
                    .pl(px(6.))
                    .pr(px(10.))
                    .py_0()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .child(
                        Icon::new(self.icon)
                            .size(px(16.))
                            .flex_none()
                            .text_color(colors.icon),
                    )
                    .child(self.text),
            )
    }
}

// -------------------------------------------------------------------- menus

/// A popover/menu surface: panel bg, 1px border, radius 10, padding 6, menu
/// shadow, vertical stack. Callers set `min_w` (170 create menus, 220 more menus).
pub fn menu_panel() -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::menu())
        .p(px(6.))
        .shadow(menu_shadow())
        .text_color(theme::fg())
        .text_size(px(13.))
}

/// A 1px separator between menu groups (margin 4px 2px).
pub fn menu_separator() -> Div {
    div()
        .h(px(1.))
        .mx(px(2.))
        .my(px(4.))
        .flex_none()
        .bg(theme::border())
}

/// A muted note/empty row inside a menu (12px, padding 6x8).
pub fn menu_label(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(8.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(theme::muted())
        .child(text.into())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuItemStyle {
    /// Create menus: hover `--hover`.
    #[default]
    Plain,
    /// "More" menus: hover solid accent + white text.
    Accent,
    /// Red text; hover red bg + white text.
    Danger,
}

/// One menu row: gap 8, padding 6x8, radius 6, 13px, optional 16px glyph or
/// emoji/image icon, optional trailing element.
#[derive(IntoElement)]
pub struct MenuItem {
    id: ElementId,
    label: SharedString,
    icon: Option<AnyElement>,
    trailing: Option<AnyElement>,
    style: MenuItemStyle,
    selected: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl MenuItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            trailing: None,
            style: MenuItemStyle::Plain,
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    /// A 16px line glyph in the current text color.
    pub fn glyph(mut self, glyph: Glyph) -> Self {
        self.icon = Some(
            Icon::new(glyph)
                .size(px(16.))
                .flex_none()
                .into_any_element(),
        );
        self
    }

    /// Any leading element (e.g. [`emoji_or_image_icon`]).
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing = Some(el.into_any_element());
        self
    }

    pub fn style(mut self, style: MenuItemStyle) -> Self {
        self.style = style;
        self
    }

    pub fn danger(self) -> Self {
        self.style(MenuItemStyle::Danger)
    }

    pub fn accent_hover(self) -> Self {
        self.style(MenuItemStyle::Accent)
    }

    /// Keyboard-highlighted row: drawn as hovered.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for MenuItem {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (fg, hover_bg, hover_fg) = match self.style {
            MenuItemStyle::Plain => (theme::fg(), theme::hover(), theme::fg()),
            MenuItemStyle::Accent => (theme::fg(), theme::accent(), theme::white()),
            MenuItemStyle::Danger => (theme::red(), theme::red(), theme::white()),
        };
        let mut el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(theme::radius::sm())
            .text_size(px(13.))
            .text_color(fg)
            .whitespace_nowrap()
            .children(self.icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.label),
            )
            .children(self.trailing);
        if self.disabled {
            return el.opacity(0.5);
        }
        if self.selected {
            el = el.bg(hover_bg).text_color(hover_fg);
        }
        el = el
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_fg));
        if let Some(handler) = self.on_click {
            el = el.on_click(handler);
        }
        el
    }
}

// ------------------------------------------------------------------ buttons

// The helpers return the kit [`Button`]: chain `.on_click(..)`,
// `.tooltip("..")` (kit tooltip), `.disabled(..)` or [`disabled`]. Labels sit
// in a child (the kit sizes its own label text) and stay the accessible name.

/// Header icon button (`.hbtn`), the kit ghost button: 28×28, radius 7, muted
/// 16px glyph → fg on the kit hover fill. Icon-only: add
/// `.accessibility_label(..)` and `.tooltip(..)`.
pub fn icon_button(id: impl Into<ElementId>, glyph: Glyph) -> Button {
    Button::new(id)
        .ghost()
        .rounded(theme::radius::tab())
        .flex_none()
        .size(px(28.))
        .p_0()
        .text_color(theme::muted())
        .cursor_pointer()
        .child(Icon::new(glyph).size(px(16.)))
}

/// Primary button (`.new-btn`), the kit primary button (accent fill, white
/// text): 13/500, h28, padding 0 12, radius 8.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    Button::new(id)
        .primary()
        .rounded(px(8.))
        .accessibility_label(label.clone())
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
}

/// Outline button (`.prop-del-row button`), the kit outline button: 1px
/// border, transparent, fg, radius 8, padding 7×14, 13px; kit hover fill.
pub fn outline_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    Button::new(id)
        .outline()
        .rounded(px(8.))
        .accessibility_label(label.clone())
        .flex_none()
        .h_auto()
        .px(px(14.))
        .py(px(7.))
        .border_color(theme::border())
        .bg(transparent_black())
        .text_color(theme::fg())
        .cursor_pointer()
        .child(div().text_size(px(13.)).child(label))
}

/// Danger button, the kit danger button drawn as an outline at rest: red
/// border and text, radius 8, padding 7×14, 13px; kit red fill + white text
/// on hover.
pub fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    Button::new(id)
        .danger()
        .rounded(px(8.))
        .accessibility_label(label.clone())
        .flex_none()
        .h_auto()
        .px(px(14.))
        .py(px(7.))
        .border_1()
        .border_color(theme::red())
        .bg(transparent_black())
        .text_color(theme::red())
        .cursor_pointer()
        .child(div().text_size(px(13.)).child(label))
}

/// Disable a kit button the Roostr way: no clicks, no hover, opacity 0.5 and
/// the default cursor (the kit keeps the rest colours).
pub fn disabled(button: Button, disabled: bool) -> Button {
    button
        .disabled(disabled)
        .when(disabled, |b| b.opacity(0.5).cursor_default())
}

// ------------------------------------------------------------ text & frames

/// A section heading: 11px, 600, uppercase, letter-spacing 0.05em, muted.
pub fn section_label(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div().flex().child(
        spaced_text(text.to_uppercase(), 0.05)
            .size(px(11.))
            .weight(FontWeight::SEMIBOLD)
            .color(theme::muted()),
    )
}

/// One line of text with CSS `letter-spacing` (GPUI text has none): the line
/// is shaped whole (kerning stays), then each character is placed at its
/// shaped x plus the accumulated spacing; like CSS, spacing also follows the
/// last character. Unset font family/size/weight/color/line height inherit
/// from the surrounding text style. Never wraps.
#[derive(IntoElement)]
pub struct SpacedText {
    runs: Vec<(String, Option<Hsla>)>,
    spacing_em: f32,
    family: Option<SharedString>,
    size: Option<Pixels>,
    weight: Option<FontWeight>,
    color: Option<Hsla>,
    line_height: Option<Pixels>,
}

/// `text` with `letter_spacing_em` × font size between characters.
pub fn spaced_text(text: impl Into<String>, letter_spacing_em: f32) -> SpacedText {
    SpacedText::new(letter_spacing_em).run(text, None)
}

impl SpacedText {
    /// Empty; add [`Self::run`]s.
    pub fn new(letter_spacing_em: f32) -> Self {
        Self {
            runs: Vec::new(),
            spacing_em: letter_spacing_em,
            family: None,
            size: None,
            weight: None,
            color: None,
            line_height: None,
        }
    }

    /// Append text; `color` `None` = the element's color.
    pub fn run(mut self, text: impl Into<String>, color: Option<Hsla>) -> Self {
        self.runs.push((text.into(), color));
        self
    }

    pub fn family(mut self, family: impl Into<SharedString>) -> Self {
        self.family = Some(family.into());
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn line_height(mut self, line_height: Pixels) -> Self {
        self.line_height = Some(line_height);
        self
    }
}

impl RenderOnce for SpacedText {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let style = window.text_style();
        let rem = window.rem_size();
        let family = self.family.unwrap_or_else(|| style.font_family.clone());
        let size = self.size.unwrap_or_else(|| style.font_size.to_pixels(rem));
        let weight = self.weight.unwrap_or(style.font_weight);
        let color = self.color.unwrap_or(style.color);
        let line_height = self.line_height.unwrap_or_else(|| {
            let mut sized = style.clone();
            sized.font_size = size.into();
            sized.line_height_in_pixels(rem)
        });
        let spacing = size * self.spacing_em;
        let font = Font {
            family: family.clone(),
            weight,
            ..style.font()
        };
        let text: String = self.runs.iter().map(|(t, _)| t.as_str()).collect();
        let runs: Vec<TextRun> = self
            .runs
            .iter()
            .filter(|(t, _)| !t.is_empty())
            .map(|(t, c)| TextRun {
                len: t.len(),
                font: font.clone(),
                color: c.unwrap_or(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        let line = window
            .text_system()
            .shape_line(text.clone().into(), size, &runs, None);
        let count = text.chars().count();
        let mut children = Vec::with_capacity(count);
        let (mut index, mut byte) = (0usize, 0usize);
        for (run, run_color) in &self.runs {
            for ch in run.chars() {
                let x = line.x_for_index(byte) + spacing * index as f32;
                children.push(
                    div()
                        .absolute()
                        .top_0()
                        .left(x)
                        .text_color(run_color.unwrap_or(color))
                        .child(ch.to_string()),
                );
                index += 1;
                byte += ch.len_utf8();
            }
        }
        div()
            .relative()
            .flex_none()
            .w(line.width() + spacing * count as f32)
            .h(line_height)
            .font_family(family)
            .font_weight(weight)
            .text_size(size)
            .line_height(line_height)
            .text_color(color)
            .whitespace_nowrap()
            .children(children)
    }
}

/// A modal panel: panel bg, 1px border, radius 14, modal shadow, `width` wide.
/// Callers add padding and the overlay. Kept for the search modal, whose
/// mobile bottom sheet (drag-to-dismiss pan) the kit `Dialog` cannot host;
/// prompts and confirms use the kit `Dialog`.
pub fn modal_frame(width: Pixels) -> Div {
    div()
        .flex()
        .flex_col()
        .w(width)
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::modal())
        .shadow(modal_shadow())
        .text_color(theme::fg())
        .overflow_hidden()
}

// ------------------------------------------------------------------ keys

/// A key binding whose keystrokes may name `secondary-`: Command on a Mac,
/// Control elsewhere - in a browser too, where a Mac's Command arrives as
/// Control (vendor/gpui-pre-web). GPUI resolves `secondary-` by the host
/// (`KeyBinding::load`); `KeyBinding::new` takes explicit modifiers only.
pub fn secondary_binding<A: Action>(
    keystrokes: &str,
    action: A,
    context: Option<&str>,
    cx: &App,
) -> KeyBinding {
    let context = context.map(|context| {
        KeyBindingContextPredicate::parse(context)
            .expect("a valid key context")
            .into()
    });
    KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("valid keystrokes")
}

// ------------------------------------------------------------------ tooltip

/// The `[data-tip]` tooltip as the kit [`Tooltip`](KitTooltip), so plain
/// elements match the kit controls' own `.tooltip(..)`. A trailing shortcut
/// hint ("Search (⌘K)") shows as a kit [`Kbd`] after the label
/// ([`split_shortcut`]). Use on a stateful element: `.tooltip(tooltip("Back"))`;
/// kit controls take the text directly: `Button::tooltip("Back")`.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    let (label, stroke) = match split_shortcut(&text) {
        Some((label, stroke)) => (SharedString::from(label.to_string()), Some(stroke)),
        None => (text.clone(), None),
    };
    move |window, cx| {
        KitTooltip::new(label.clone())
            .key_binding(stroke.clone().map(Kbd::new))
            .build(window, cx)
    }
}

/// A tooltip text's trailing shortcut hint: "Search (⌘K)" → `("Search", ⌘K)`
/// (⌘ is Ctrl off macOS). `None` without a modifier glyph in the parentheses.
pub fn split_shortcut(text: &str) -> Option<(&str, Keystroke)> {
    let (label, hint) = text.strip_suffix(')')?.rsplit_once(" (")?;
    let mut modifiers = Modifiers::default();
    let mut key = String::new();
    for c in hint.chars() {
        match c {
            '⌘' if cfg!(target_os = "macos") => modifiers.platform = true,
            '⌘' | '⌃' => modifiers.control = true,
            '⌥' => modifiers.alt = true,
            '⇧' => modifiers.shift = true,
            c => key.extend(c.to_lowercase()),
        }
    }
    if !modifiers.modified() || key.is_empty() {
        return None;
    }
    let stroke = Keystroke {
        modifiers,
        key,
        key_char: None,
    };
    Some((label, stroke))
}
