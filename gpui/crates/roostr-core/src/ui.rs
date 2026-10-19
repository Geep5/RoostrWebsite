//! Shared UI primitives, styled per `.specs/roostr/style.md` §5 so every
//! feature crate looks the same: checkbox and property icons, tag pills,
//! property badges, popover menus, header/primary/outline buttons, section
//! labels, modal frames, tooltips and emoji-or-image icons.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use base64::Engine as _;
use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::icons::Glyph;
use crate::objects::is_image_icon;
use crate::options::{badge_colors, tag_colors, unknown_tag_bg};
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

/// Anytype's checkbox (`CheckboxIcon.svelte`): 20px default, muted
/// unchecked, accent checked.
#[derive(IntoElement)]
pub struct CheckboxIcon {
    checked: bool,
    size: Pixels,
    color: Option<Hsla>,
}

impl CheckboxIcon {
    pub fn new(checked: bool) -> Self {
        Self {
            checked,
            size: px(20.),
            color: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// Override the state color (e.g. `fg` on hover).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for CheckboxIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let glyph = if self.checked {
            Glyph::CheckboxChecked
        } else {
            Glyph::CheckboxUnchecked
        };
        let color = self.color.unwrap_or_else(|| {
            if self.checked {
                theme::accent()
            } else {
                theme::muted()
            }
        });
        Icon::new(glyph)
            .size(self.size)
            .flex_none()
            .text_color(color)
    }
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

/// An option pill (Anytype tagItem): h20, padding 0 6, radius 10, 12px; bg/text
/// from the palette, unknown colour `rgba(255,255,255,0.09)` with primary text.
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
        let (bg, fg) = tag_colors(&self.color).unwrap_or((unknown_tag_bg(), theme::fg()));
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(20.))
            .px(px(6.))
            .rounded(px(10.))
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(self.text)
    }
}

/// A property badge (`ObjectMeta .badge`): h26, padding 0 10 0 6, gap 6,
/// radius 8, 13/500, a leading 16px glyph in the badge's icon color; hover
/// brightness 1.18.
#[derive(IntoElement)]
pub struct Badge {
    id: ElementId,
    text: SharedString,
    color: SharedString,
    icon: Glyph,
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
            on_click: None,
        }
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
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let colors = badge_colors(&self.color);
        let el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .pl(px(6.))
            .pr(px(10.))
            .rounded(px(8.))
            .bg(colors.bg)
            .text_color(colors.fg)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .hover(move |s| s.bg(brighten(colors.bg, 1.18)))
            .child(
                Icon::new(self.icon)
                    .size(px(16.))
                    .flex_none()
                    .text_color(colors.icon),
            )
            .child(self.text);
        match self.on_click {
            Some(handler) => el.cursor_pointer().on_click(handler),
            None => el,
        }
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

/// Header icon button (`.hbtn`): 28×28, radius 7, muted 16px glyph → fg on a
/// `--hover` fill. Chain `.on_click(...)` / `.tooltip(tooltip("..."))`.
pub fn icon_button(id: impl Into<ElementId>, glyph: Glyph) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(theme::radius::tab())
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
        .child(Icon::new(glyph).size(px(16.)))
}

/// Primary button (`.new-btn`): accent fill, white 13/500 text, h28, padding 0 12,
/// radius 8; hover brightness 1.1.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(12.))
        .rounded(px(8.))
        .bg(theme::accent())
        .text_color(theme::white())
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(brighten(theme::accent(), 1.1)))
        .child(label.into())
}

/// Outline button (`.prop-del-row button`): 1px border, transparent, fg,
/// radius 8, padding 7×14, 13px. Pass `disabled` for opacity 0.5 (no hover).
pub fn outline_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    outline_base(id)
        .hover(|s| s.bg(theme::hl_med()))
        .child(label.into())
}

fn outline_base(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(14.))
        .py(px(7.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme::border())
        .text_color(theme::fg())
        .text_size(px(13.))
        .whitespace_nowrap()
        .cursor_pointer()
}

/// Danger outline: red border and text; hover red fill + white text.
pub fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    outline_base(id)
        .border_color(theme::red())
        .text_color(theme::red())
        .hover(|s| s.bg(theme::red()).text_color(theme::white()))
        .child(label.into())
}

/// Dim a button and drop its pointer when disabled (opacity 0.5).
pub fn disabled(el: Stateful<Div>, disabled: bool) -> Stateful<Div> {
    if disabled {
        el.opacity(0.5).cursor_default()
    } else {
        el
    }
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
/// Callers add padding (settings: 18 22 22) and the overlay.
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

/// A small modal (`.prop-del-modal`): radius 12, padding 20.
pub fn small_modal_frame(width: Pixels) -> Div {
    modal_frame(width).rounded(px(12.)).p(px(20.))
}

/// The modal backdrop: full-size `rgb(0 0 0/0.5)`, content centered.
pub fn modal_overlay() -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::overlay())
}

// ------------------------------------------------------------------ tooltip

struct TextTooltip(SharedString);

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The pad keeps the shadow inside the tooltip's bounds and sets it 6px off the control.
        div().pt(px(6.)).child(
            div()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(theme::radius::sm())
                .px(px(8.))
                .py(px(3.))
                .text_size(px(11.))
                .text_color(theme::fg())
                .whitespace_nowrap()
                .shadow(tooltip_shadow())
                .child(self.0.clone()),
        )
    }
}

/// The `[data-tip]` tooltip: panel bg, 1px border, radius 6, padding 3×8,
/// 11px, tooltip shadow. Use on a stateful element: `.tooltip(tooltip("Back"))`.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| TextTooltip(text.clone())).into()
}
