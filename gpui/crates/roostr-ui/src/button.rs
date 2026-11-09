//! Buttons: gpui-kit's `Button` (every variant, size and state), `Toggle` and `ToggleGroup`,
//! and Roostr's button helpers from `roostr_core::ui` ([`icon_button`], [`primary_button`],
//! [`outline_button`], [`danger_button`], [`disabled`]).
//!
//! Metrics are gpui-kit's, in rems of 14px; colours are `t.kit.*` resolved as gpui-kit did.
//! Rune text does not inherit a colour: a button paints its label and icon itself; extra
//! `.child(..)` elements keep their own colours.

use rune::{Color, Cursor, El, Span, Tx, row, text};

use crate::display::Spinner;

use crate::icons::{Glyph, glyph};
use crate::overlay::Acts;
use crate::theme::{Theme, radius, rems, shadow, ty};
use crate::ty::Weights as _;

/// gpui-kit's `Size`: the shared component size (`Sizable::with_size`, `.xsmall()`, …).
/// `Px` is gpui-kit's `Size::Size(px)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum KitSize {
    XSmall,
    Small,
    #[default]
    Medium,
    Large,
    Px(f32),
}

impl KitSize {
    /// gpui-kit's `Icon::with_size`: `size_3`, `size_3p5`, `size_4`, `size_6` (10.5, 12.25, 14,
    /// 21), or the given size.
    pub fn icon_px(self) -> f32 {
        match self {
            KitSize::XSmall => rems(0.75),
            KitSize::Small => rems(0.875),
            KitSize::Medium => rems(1.0),
            KitSize::Large => rems(1.5),
            KitSize::Px(px) => px,
        }
    }
}

/// gpui-kit's `Colorize::opacity`: alpha scaled by `f` (clamped to 0..=1).
pub(crate) fn opacity(c: Color, f: f32) -> Color {
    c.alpha(c.a * f.clamp(0.0, 1.0))
}

/// gpui-kit's `ButtonCustomVariant`: the fill, label colour, hover and pressed fills and
/// whether the button casts `shadow_xs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonColors {
    pub color: Color,
    pub foreground: Color,
    pub hover: Color,
    pub active: Color,
    pub shadow: bool,
}

/// Builds [`ButtonColors`] as gpui-kit's `ButtonCustomVariant::new(cx)` did: transparent
/// fills, the kit foreground, no shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonCustomVariant(ButtonColors);

impl ButtonCustomVariant {
    pub fn new(t: &Theme) -> Self {
        Self(ButtonColors {
            color: Color::TRANSPARENT,
            foreground: t.kit.foreground,
            hover: Color::TRANSPARENT,
            active: Color::TRANSPARENT,
            shadow: false,
        })
    }
    pub fn color(mut self, c: Color) -> Self {
        self.0.color = c;
        self
    }
    pub fn foreground(mut self, c: Color) -> Self {
        self.0.foreground = c;
        self
    }
    pub fn hover(mut self, c: Color) -> Self {
        self.0.hover = c;
        self
    }
    pub fn active(mut self, c: Color) -> Self {
        self.0.active = c;
        self
    }
    pub fn shadow(mut self, shadow: bool) -> Self {
        self.0.shadow = shadow;
        self
    }
    pub fn colors(self) -> ButtonColors {
        self.0
    }
}

impl From<ButtonCustomVariant> for ButtonColors {
    fn from(v: ButtonCustomVariant) -> Self {
        v.0
    }
}

/// gpui-kit's `ButtonVariant`. `Default` is the kit's bordered default button.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Primary,
    Secondary,
    Danger,
    Info,
    Success,
    Warning,
    Ghost,
    Link,
    Text,
    Custom(ButtonColors),
}

/// gpui-kit's `ButtonRounded`: half, one or two kit radii, none, or a radius in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ButtonRounded {
    None,
    Small,
    #[default]
    Medium,
    Large,
    Px(f32),
}

impl ButtonRounded {
    fn px(self) -> f32 {
        match self {
            ButtonRounded::None => 0.0,
            ButtonRounded::Small => radius::KIT * 0.5,
            ButtonRounded::Medium => radius::KIT,
            ButtonRounded::Large => radius::KIT * 2.0,
            ButtonRounded::Px(r) => r,
        }
    }
}

/// One state's paint (gpui-kit's `ButtonVariantStyle`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    bg: Color,
    border: Color,
    fg: Color,
    underline: bool,
    shadow: bool,
}

#[derive(Clone, Copy)]
enum State {
    Normal,
    Hovered,
    Active,
}

impl ButtonVariant {
    fn no_padding(self) -> bool {
        matches!(self, Self::Link | Self::Text)
    }

    fn outline_bg(self, state: State, t: &Theme) -> Color {
        let k = &t.kit;
        let pick = |n: Color, h: Color, a: Color| match state {
            State::Normal => opacity(n, 0.1),
            State::Hovered => opacity(h, 0.2),
            State::Active => opacity(a, 0.4),
        };
        match self {
            Self::Default => match state {
                State::Normal => input_background(t),
                State::Hovered => opacity(k.input, 0.5),
                State::Active => opacity(k.input, 0.7),
            },
            Self::Primary => pick(k.primary, k.primary_hover, k.primary_active),
            Self::Secondary => pick(k.secondary, k.secondary_hover, k.secondary_active),
            Self::Danger => pick(k.danger, k.danger_hover, k.danger_active),
            Self::Warning => pick(k.warning, k.warning_hover, k.warning_active),
            Self::Success => pick(k.success, k.success_hover, k.success_active),
            Self::Info => pick(k.info, k.info_hover, k.info_active),
            Self::Ghost | Self::Link | Self::Text => Color::TRANSPARENT,
            Self::Custom(c) => opacity(c.color, 0.2),
        }
    }

    fn bg(self, outline: bool, t: &Theme) -> Color {
        if outline {
            return self.outline_bg(State::Normal, t);
        }
        let k = &t.kit;
        match self {
            Self::Default => k.button,
            Self::Primary => k.button_primary,
            Self::Secondary => k.button_secondary,
            Self::Danger => k.button_danger,
            Self::Warning => k.button_warning,
            Self::Success => k.button_success,
            Self::Info => k.button_info,
            Self::Ghost | Self::Link | Self::Text => Color::TRANSPARENT,
            Self::Custom(c) => opacity(c.color, 0.2),
        }
    }

    fn fg(self, outline: bool, t: &Theme) -> Color {
        let k = &t.kit;
        let o = |a: Color, b: Color| if outline { a } else { b };
        match self {
            Self::Default => k.button_foreground,
            Self::Primary => o(k.primary, k.button_primary_foreground),
            Self::Secondary => o(k.secondary_foreground, k.button_secondary_foreground),
            Self::Ghost => k.secondary_foreground,
            Self::Danger => o(k.danger, k.button_danger_foreground),
            Self::Warning => o(k.warning, k.button_warning_foreground),
            Self::Success => o(k.success, k.button_success_foreground),
            Self::Info => o(k.info, k.button_info_foreground),
            Self::Link => k.link,
            Self::Text => opacity(k.foreground, 0.9),
            Self::Custom(c) => c.foreground,
        }
    }

    fn border(self, outline: bool, t: &Theme) -> Color {
        let k = &t.kit;
        let o = |vivid: Color, fill: Color| if outline { opacity(vivid, 0.4) } else { fill };
        match self {
            Self::Default => k.input,
            Self::Secondary => k.border,
            Self::Primary => k.primary,
            Self::Danger => o(k.danger, k.button_danger),
            Self::Info => o(k.info, k.button_info),
            Self::Warning => o(k.warning, k.button_warning),
            Self::Success => o(k.success, k.button_success),
            Self::Ghost | Self::Link | Self::Text => Color::TRANSPARENT,
            Self::Custom(c) => o(c.color, c.color),
        }
    }

    fn shadow(self) -> bool {
        matches!(self, Self::Custom(c) if c.shadow)
    }

    fn look(self, bg: Color, outline: bool, fg: Color, t: &Theme) -> Look {
        Look {
            bg,
            border: self.border(outline, t),
            fg,
            underline: matches!(self, Self::Link),
            shadow: self.shadow(),
        }
    }

    fn normal(self, outline: bool, t: &Theme) -> Look {
        self.look(self.bg(outline, t), outline, self.fg(outline, t), t)
    }

    fn hovered(self, outline: bool, t: &Theme) -> Look {
        let k = &t.kit;
        let ob = |fill: Color| {
            if outline {
                self.outline_bg(State::Hovered, t)
            } else {
                fill
            }
        };
        let bg = match self {
            Self::Default => ob(k.button_hover),
            Self::Primary => ob(k.button_primary_hover),
            Self::Secondary => ob(k.button_secondary_hover),
            Self::Danger => ob(k.button_danger_hover),
            Self::Warning => ob(k.button_warning_hover),
            Self::Success => ob(k.button_success_hover),
            Self::Info => ob(k.button_info_hover),
            Self::Custom(c) => c.hover,
            Self::Ghost => {
                if t.dark {
                    opacity(k.accent, 0.5)
                } else {
                    k.accent
                }
            }
            Self::Link | Self::Text => Color::TRANSPARENT,
        };
        let fg = match self {
            Self::Link => k.link_hover,
            Self::Text => k.foreground,
            Self::Ghost => k.accent_foreground,
            _ => self.fg(outline, t),
        };
        self.look(bg, outline, fg, t)
    }

    fn active(self, outline: bool, t: &Theme) -> Look {
        let k = &t.kit;
        let ob = |fill: Color| {
            if outline {
                self.outline_bg(State::Active, t)
            } else {
                fill
            }
        };
        let bg = match self {
            Self::Default => ob(k.button_active),
            Self::Primary => ob(k.button_primary_active),
            Self::Secondary => ob(k.button_secondary_active),
            Self::Ghost => k.button_active,
            Self::Danger => ob(k.button_danger_active),
            Self::Warning => ob(k.button_warning_active),
            Self::Success => ob(k.button_success_active),
            Self::Info => ob(k.button_info_active),
            Self::Custom(c) => c.active,
            Self::Link | Self::Text => Color::TRANSPARENT,
        };
        let fg = match self {
            Self::Link => k.link_active,
            Self::Text => opacity(k.foreground, 0.7),
            _ => self.fg(outline, t),
        };
        self.look(bg, outline, fg, t)
    }

    fn selected(self, outline: bool, t: &Theme) -> Look {
        if outline {
            return Look {
                fg: self.fg(true, t),
                ..self.active(true, t)
            };
        }
        let k = &t.kit;
        let bg = match self {
            Self::Default => k.button_active,
            Self::Primary => k.button_primary_active,
            Self::Secondary => k.button_secondary_active,
            Self::Ghost => k.secondary_active,
            Self::Danger => k.button_danger_active,
            Self::Warning => k.button_warning_active,
            Self::Success => k.button_success_active,
            Self::Info => k.button_info_active,
            Self::Link | Self::Text => Color::TRANSPARENT,
            Self::Custom(c) => c.active,
        };
        let fg = match self {
            Self::Link => k.link_active,
            Self::Text => opacity(k.foreground, 0.7),
            _ => self.fg(false, t),
        };
        self.look(bg, false, fg, t)
    }

    fn disabled(self, outline: bool, t: &Theme) -> Look {
        let k = &t.kit;
        let fg = opacity(k.muted_foreground, 0.5);
        let (bg, border) = if outline {
            (
                opacity(self.outline_bg(State::Normal, t), 0.5),
                opacity(self.border(true, t), 0.5),
            )
        } else {
            match self {
                Self::Default => (opacity(input_background(t), 0.5), opacity(k.input, 0.5)),
                Self::Link | Self::Ghost | Self::Text => (Color::TRANSPARENT, Color::TRANSPARENT),
                Self::Primary => (
                    opacity(k.button_primary, 0.15),
                    opacity(k.button_primary, 0.15),
                ),
                Self::Danger => (
                    opacity(k.button_danger, 0.15),
                    opacity(k.button_danger, 0.15),
                ),
                Self::Warning => (
                    opacity(k.button_warning, 0.15),
                    opacity(k.button_warning, 0.15),
                ),
                Self::Success => (
                    opacity(k.button_success, 0.15),
                    opacity(k.button_success, 0.15),
                ),
                Self::Info => (opacity(k.button_info, 0.15), opacity(k.button_info, 0.15)),
                Self::Secondary => (k.button_secondary, k.button_secondary),
                Self::Custom(c) => (opacity(c.color, 0.15), opacity(c.color, 0.15)),
            }
        };
        Look {
            bg,
            border,
            fg,
            underline: matches!(self, Self::Link),
            shadow: false,
        }
    }
}

/// gpui-kit's `Theme::input_background`: the input colour at 30% when dark, else the kit
/// background.
pub(crate) fn input_background(t: &Theme) -> Color {
    if t.dark {
        opacity(t.kit.input, 0.3)
    } else {
        t.kit.background
    }
}

/// Caller overrides of a control's frame, replayed over every state as gpui-kit's
/// `refine_style(&instance_style)` did (hover still wins, as in GPUI).
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    w: Option<f32>,
    h: Option<f32>,
    h_auto: bool,
    min_w: Option<f32>,
    pad: [Option<f32>; 4],
    radius: Option<f32>,
    fill: Option<Color>,
    fg: Option<Color>,
    border: Option<Color>,
    border_w: Option<f32>,
    text_size: Option<f32>,
    weight: Option<f32>,
    cursor: Option<Cursor>,
    gap: Option<f32>,
    /// Theme-token overrides (Roostr's helpers name `theme::muted()` etc. before a theme is
    /// at hand): rest text colour and border colour.
    fg_of: Option<fn(&Theme) -> Color>,
    border_of: Option<fn(&Theme) -> Color>,
}

macro_rules! frame_methods {
    () => {
        /// Width in points (`.w(px(..))`).
        pub fn w(mut self, w: f32) -> Self {
            self.frame.w = Some(w);
            self
        }
        /// Height in points (`.h(px(..))`).
        pub fn h(mut self, h: f32) -> Self {
            self.frame.h = Some(h);
            self.frame.h_auto = false;
            self
        }
        /// Width and height in points (`.size(px(..))`).
        pub fn square(self, s: f32) -> Self {
            self.w(s).h(s)
        }
        /// Content height (`.h_auto()`).
        pub fn h_auto(mut self) -> Self {
            self.frame.h = None;
            self.frame.h_auto = true;
            self
        }
        pub fn min_w(mut self, w: f32) -> Self {
            self.frame.min_w = Some(w);
            self
        }
        /// Padding on all sides (`.p(..)`, `.p_0()` = 0).
        pub fn p(mut self, p: f32) -> Self {
            self.frame.pad = [Some(p); 4];
            self
        }
        pub fn px(mut self, p: f32) -> Self {
            self.frame.pad[0] = Some(p);
            self.frame.pad[2] = Some(p);
            self
        }
        pub fn py(mut self, p: f32) -> Self {
            self.frame.pad[1] = Some(p);
            self.frame.pad[3] = Some(p);
            self
        }
        pub fn pl(mut self, p: f32) -> Self {
            self.frame.pad[0] = Some(p);
            self
        }
        pub fn pr(mut self, p: f32) -> Self {
            self.frame.pad[2] = Some(p);
            self
        }
        /// Corner radius in points (`.rounded(px(..))`).
        pub fn radius(mut self, r: f32) -> Self {
            self.frame.radius = Some(r);
            self
        }
        /// Fill at rest (`.bg(..)`).
        pub fn bg(mut self, c: Color) -> Self {
            self.frame.fill = Some(c);
            self
        }
        /// Label and icon colour at rest (`.text_color(..)`).
        pub fn text_color(mut self, c: Color) -> Self {
            self.frame.fg = Some(c);
            self
        }
        pub fn border_color(mut self, c: Color) -> Self {
            self.frame.border = Some(c);
            self
        }
        /// Border width (`.border_1()` = 1, `.border_0()` = 0).
        pub fn border_w(mut self, w: f32) -> Self {
            self.frame.border_w = Some(w);
            self
        }
        pub fn text_size(mut self, s: f32) -> Self {
            self.frame.text_size = Some(s);
            self
        }
        pub fn weight(mut self, w: f32) -> Self {
            self.frame.weight = Some(w);
            self
        }
        pub fn cursor(mut self, c: Cursor) -> Self {
            self.frame.cursor = Some(c);
            self
        }
        /// `.cursor_pointer()`.
        pub fn pointer(self) -> Self {
            self.cursor(Cursor::Pointer)
        }
        /// Gap between icon, label and children.
        pub fn gap(mut self, g: f32) -> Self {
            self.frame.gap = Some(g);
            self
        }
    };
}

/// gpui-kit's `Button`: `Button::new("save").label("Save").primary().on_press(..).build(t)`.
pub struct Button {
    key: String,
    label: Option<String>,
    a11y: Option<String>,
    icon: Option<Glyph>,
    icon_size: Option<f32>,
    children: Vec<El>,
    loading: bool,
    selected: bool,
    disabled: bool,
    dim: bool,
    tooltip: Option<String>,
    compact: bool,
    outline: bool,
    rounded: ButtonRounded,
    size: KitSize,
    variant: ButtonVariant,
    frame: Frame,
    acts: Acts,
}

impl Button {
    pub fn new(key: impl Into<String>) -> Self {
        Button {
            key: key.into(),
            label: None,
            a11y: None,
            icon: None,
            icon_size: None,
            children: Vec::new(),
            loading: false,
            selected: false,
            disabled: false,
            dim: false,
            tooltip: None,
            compact: false,
            outline: false,
            rounded: ButtonRounded::Medium,
            size: KitSize::Medium,
            variant: ButtonVariant::Default,
            frame: Frame::default(),
            acts: Acts::default(),
        }
    }

    frame_methods!();

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The accessible name when the visible content is not it.
    pub fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.a11y = Some(label.into());
        self
    }
    /// A leading icon; alone it makes an icon button (square).
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    /// The icon's size in points (gpui-kit: the button size's icon size).
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = Some(s);
        self
    }
    /// Any content after the label (paint its colours yourself).
    pub fn child(mut self, el: El) -> Self {
        self.children.push(el);
        self
    }
    /// Spinner in place of the icon, dimmed to 0.8, unpressable.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// The kit tooltip ("Save (⌘S)" shows the shortcut as keycaps).
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }
    pub fn rounded(mut self, r: ButtonRounded) -> Self {
        self.rounded = r;
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn xsmall(self) -> Self {
        self.size(KitSize::XSmall)
    }
    pub fn small(self) -> Self {
        self.size(KitSize::Small)
    }
    pub fn large(self) -> Self {
        self.size(KitSize::Large)
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }
    pub fn secondary(self) -> Self {
        self.variant(ButtonVariant::Secondary)
    }
    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }
    pub fn warning(self) -> Self {
        self.variant(ButtonVariant::Warning)
    }
    pub fn success(self) -> Self {
        self.variant(ButtonVariant::Success)
    }
    pub fn info(self) -> Self {
        self.variant(ButtonVariant::Info)
    }
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }
    pub fn link(self) -> Self {
        self.variant(ButtonVariant::Link)
    }
    pub fn text(self) -> Self {
        self.variant(ButtonVariant::Text)
    }
    pub fn custom(self, colors: impl Into<ButtonColors>) -> Self {
        self.variant(ButtonVariant::Custom(colors.into()))
    }
    /// gpui-kit's `on_click`.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    fn interactive(&self) -> bool {
        !(self.disabled || self.loading)
    }

    pub fn build(self, t: &Theme) -> El {
        let v = self.variant;
        let f = self.frame;
        let interactive = self.interactive();
        let look = if self.disabled {
            v.disabled(self.outline, t)
        } else if self.selected {
            v.selected(self.outline, t)
        } else {
            v.normal(self.outline, t)
        };
        let bg = f.fill.unwrap_or(look.bg);
        let fg = f.fg.or(f.fg_of.map(|c| c(t))).unwrap_or(look.fg);
        let border = f
            .border
            .or(f.border_of.map(|c| c(t)))
            .unwrap_or(look.border);
        let icon_only = self.label.is_none() && self.children.is_empty();

        let mut e =
            row()
                .key(self.key.clone())
                .center()
                .role(if matches!(v, ButtonVariant::Link) {
                    "link"
                } else {
                    "button"
                });
        // Size: icon buttons are square; others take height and padding by size.
        if !v.no_padding() {
            if icon_only {
                let s = match self.size {
                    KitSize::Px(px) => px,
                    KitSize::XSmall => rems(1.25),
                    KitSize::Small => rems(1.5),
                    KitSize::Medium | KitSize::Large => rems(2.0),
                };
                e = e.size(s, s);
            } else {
                let (h, px, min_w) = match self.size {
                    KitSize::Px(s) => (None, s * 0.2, None),
                    KitSize::XSmall => (Some(rems(1.25)), rems(0.25), Some(rems(1.25))),
                    KitSize::Small => (
                        Some(rems(1.5)),
                        if self.compact { rems(0.375) } else { rems(0.5) },
                        Some(rems(1.5)),
                    ),
                    KitSize::Medium => (
                        Some(rems(2.0)),
                        if self.compact { rems(0.5) } else { rems(0.625) },
                        Some(rems(2.0)),
                    ),
                    KitSize::Large => (
                        Some(rems(2.0)),
                        if self.compact { rems(0.5) } else { rems(0.75) },
                        Some(rems(2.0)),
                    ),
                };
                if let Some(h) = h {
                    e = e.h(h);
                }
                e = e.px(px);
                if self.compact
                    && let Some(m) = min_w
                {
                    e = e.min_w(m);
                }
            }
        }
        e = apply_frame(e, &f);
        let r = f.radius.unwrap_or(self.rounded.px());
        let border_w =
            f.border_w
                .unwrap_or(if matches!(v, ButtonVariant::Default) || self.outline {
                    1.0
                } else {
                    0.0
                });
        e = e.radius(r).fill(bg);
        if border_w > 0.0 {
            e = e.border(border_w, border).border_layout();
        }
        // gpui-kit's `theme.shadow` is on in Roostr's kit theme.
        if !self.disabled && look.shadow {
            e = e.box_shadow(shadow::kit_xs());
        }

        // Content: icon (or spinner), label, children.
        let size_text = match self.size {
            KitSize::XSmall => rems(0.75),
            KitSize::Small | KitSize::Medium => rems(0.875),
            KitSize::Large => rems(1.0),
            KitSize::Px(s) => s * 0.875,
        };
        let ts = f.text_size.unwrap_or(size_text);
        let font = ty::ui(ts)
            .lh(ts * 1.25)
            .weight(f.weight.unwrap_or(ty::NORMAL));
        let hover = (interactive && !self.selected).then(|| v.hovered(self.outline, t));
        let hover_fg = hover.map(|h| h.fg).filter(|h| *h != fg);
        let gap = f.gap.unwrap_or(match self.size {
            KitSize::XSmall | KitSize::Small => rems(0.25),
            _ => rems(0.5),
        });
        let mut content = row()
            .key(format!("{}-label", self.key))
            .center()
            .gap(gap)
            .h_full();
        if let Some(g) = self.icon {
            let s = self.icon_size.unwrap_or(match self.size {
                KitSize::Px(v) => v * 0.75,
                s => s.icon_px(),
            });
            let mut ic = if self.loading {
                Spinner::new().px(s).color(fg).build(t)
            } else {
                glyph(g, s).color(fg)
            };
            if let Some(h) = hover_fg {
                ic = ic.group_color(h);
            }
            content = content.child(ic);
        }
        if let Some(label) = &self.label {
            let mut l = if look.underline {
                let n = label.len();
                text(label.clone()).spans(vec![Span::new(0, n).underline(fg)])
            } else {
                text(label.clone())
            };
            l = l.font(font).color(fg).truncate();
            if let Some(h) = hover_fg {
                l = l.group_color(h);
            }
            content = content.child(l);
        }
        content = content.children(self.children);
        e = e.child(content);

        if let Some(h) = hover {
            let a = v.active(self.outline, t);
            e = e.hover(h.bg).press(a.bg);
            if hover_fg.is_some() {
                e = e.hover_group();
            }
        }
        let cursor = f.cursor.unwrap_or(if interactive && v.no_padding() {
            Cursor::Pointer
        } else {
            Cursor::Default
        });
        e = e.cursor(if self.disabled {
            Cursor::Default
        } else {
            cursor
        });
        if let Some(l) = self.a11y.or(self.label) {
            e = e.label(l);
        }
        e = e.disabled(self.disabled);
        if self.loading && !self.disabled {
            e = e.layer_opacity(0.8);
        }
        if self.dim {
            e = e.layer_opacity(0.5);
        }
        if let Some(tip) = self.tooltip {
            e = crate::tooltip::tooltip(e, tip);
        }
        self.acts.apply(e, interactive)
    }
}

fn apply_frame(mut e: El, f: &Frame) -> El {
    if let Some(w) = f.w {
        e = e.w(w);
    }
    if let Some(h) = f.h {
        e = e.h(h);
    }
    if let Some(m) = f.min_w {
        e = e.min_w(m);
    }
    if let Some(p) = f.pad[0] {
        e = e.pl(p);
    }
    if let Some(p) = f.pad[1] {
        e = e.pt(p);
    }
    if let Some(p) = f.pad[2] {
        e = e.pr(p);
    }
    if let Some(p) = f.pad[3] {
        e = e.pb(p);
    }
    e
}

/// Header icon button (`.hbtn`, `roostr_core::ui::icon_button`), the kit ghost button: 28×28,
/// radius 7, a muted 16px glyph → the kit hover fill and accent foreground. Add
/// `.accessibility_label(..)` and `.tooltip(..)`.
pub fn icon_button(key: impl Into<String>, g: Glyph) -> Button {
    Button::new(key)
        .ghost()
        .radius(radius::TAB)
        .square(28.0)
        .p(0.0)
        .icon(g)
        .icon_size(16.0)
        .pointer()
        .themed(Some(|t: &Theme| t.muted), None)
}

impl Button {
    /// Rest text / border colours taken from the theme at build.
    fn themed(
        mut self,
        fg: Option<fn(&Theme) -> Color>,
        border: Option<fn(&Theme) -> Color>,
    ) -> Self {
        self.frame.fg_of = fg;
        self.frame.border_of = border;
        self
    }
}

/// Primary button (`.new-btn`, `roostr_core::ui::primary_button`): the kit primary button,
/// 13/500, h28, padding 0 12, radius 8.
pub fn primary_button(key: impl Into<String>, label: impl Into<String>) -> Button {
    Button::new(key)
        .primary()
        .radius(8.0)
        .h(28.0)
        .px(12.0)
        .pointer()
        .label(label)
        .text_size(13.0)
        .weight(ty::MEDIUM)
}

/// Outline button (`.prop-del-row button`, `roostr_core::ui::outline_button`): the kit outline
/// button, 1px border, transparent, fg text, radius 8, padding 7×14, 13px; kit hover fill.
pub fn outline_button(key: impl Into<String>, label: impl Into<String>) -> Button {
    Button::new(key)
        .outline()
        .radius(8.0)
        .h_auto()
        .px(14.0)
        .py(7.0)
        .bg(Color::TRANSPARENT)
        .pointer()
        .label(label)
        .text_size(13.0)
        .themed(Some(|t: &Theme| t.fg), Some(|t: &Theme| t.border))
}

/// Danger button (`roostr_core::ui::danger_button`): the kit danger button drawn as an outline at
/// rest (red border and text, radius 8, padding 7×14, 13px), kit red fill + white text on hover.
pub fn danger_button(key: impl Into<String>, label: impl Into<String>) -> Button {
    Button::new(key)
        .danger()
        .radius(8.0)
        .h_auto()
        .px(14.0)
        .py(7.0)
        .border_w(1.0)
        .bg(Color::TRANSPARENT)
        .pointer()
        .label(label)
        .text_size(13.0)
        .themed(Some(|t: &Theme| t.red), Some(|t: &Theme| t.red))
}

/// Disable a button the Roostr way (`roostr_core::ui::disabled`): no presses, no hover, the rest
/// colours at opacity 0.5 and the default cursor.
pub fn disabled(button: Button, disabled: bool) -> Button {
    let mut b = button.disabled(disabled);
    b.dim = disabled;
    b
}

/// gpui-kit's `ToggleVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleVariant {
    #[default]
    Ghost,
    Outline,
}

type ToggleAct = Box<dyn FnOnce(El, bool) -> El>;

/// gpui-kit's `Toggle`: a pressable that is on or off. Off it is transparent (outline: kit
/// background and border) with the kit accent on hover; on it is the kit accent. Caller
/// overrides (`.bg`, `.text_color`, …) win over both, as in gpui-kit.
pub struct Toggle {
    key: String,
    checked: bool,
    size: KitSize,
    variant: ToggleVariant,
    disabled: bool,
    corners: [bool; 4],
    edges: [bool; 4],
    label: Option<String>,
    icon: Option<Glyph>,
    tooltip: Option<String>,
    frame: Frame,
    on_toggle: Option<ToggleAct>,
}

impl Toggle {
    pub fn new(key: impl Into<String>) -> Self {
        Toggle {
            key: key.into(),
            checked: false,
            size: KitSize::Medium,
            variant: ToggleVariant::Ghost,
            disabled: false,
            corners: [true; 4],
            edges: [true; 4],
            label: None,
            icon: None,
            tooltip: None,
            frame: Frame::default(),
            on_toggle: None,
        }
    }

    frame_methods!();

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn variant(mut self, v: ToggleVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn outline(self) -> Self {
        self.variant(ToggleVariant::Outline)
    }
    pub fn ghost(self) -> Self {
        self.variant(ToggleVariant::Ghost)
    }
    /// Round all corners fully (`.rounded_full()`).
    pub fn rounded_full(self) -> Self {
        self.radius(f32::MAX / 4.0)
    }
    /// gpui-kit's `on_click(|next: &bool, ..|)`: `f` gets the state a press asks for.
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_toggle = Some(Box::new(move |e: El, next: bool| {
            e.on_press(move |s: &mut S| f(s, next))
        }));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let k = &t.kit;
        let f = self.frame;
        let (min_w, h, px, ts) = match self.size {
            KitSize::XSmall => (rems(1.25), rems(1.25), rems(0.125), rems(0.75)),
            KitSize::Small => (rems(1.5), rems(1.5), rems(0.25), rems(0.875)),
            KitSize::Large => (rems(2.25), rems(2.25), rems(0.75), rems(1.0)),
            KitSize::Medium | KitSize::Px(_) => (rems(2.0), rems(2.0), rems(0.5), rems(0.875)),
        };
        let outline = self.variant == ToggleVariant::Outline;
        let (rest_bg, rest_fg) = if self.checked {
            (k.accent, k.accent_foreground)
        } else if outline {
            (k.background, k.foreground)
        } else {
            (Color::TRANSPARENT, k.foreground)
        };
        let bg = f.fill.unwrap_or(rest_bg);
        let fg = f.fg.unwrap_or(rest_fg);
        let ts = f.text_size.unwrap_or(ts);
        let mut e = row()
            .key(self.key.clone())
            .center()
            .role("button")
            .checked(self.checked)
            .min_w(min_w)
            .px(px);
        if !f.h_auto {
            e = e.h(h);
        }
        e = apply_frame(e, &f);
        let r = f.radius.unwrap_or(radius::KIT);
        let c = self.corners;
        e = e
            .corners([c[0], c[1], c[2], c[3]].map(|on| if on { r } else { 0.0 }))
            .fill(bg);
        let bw = f.border_w.unwrap_or(if outline { 1.0 } else { 0.0 });
        if bw > 0.0 {
            let b = f.border.unwrap_or(k.border);
            e = e
                .border_sides(self.edges.map(|on| if on { bw } else { 0.0 }), b)
                .border_layout();
        }
        let hoverable = !self.disabled && !self.checked;
        let mut content = row().center().gap(f.gap.unwrap_or(0.0));
        if let Some(g) = self.icon {
            let mut i = glyph(g, self.size.icon_px()).color(fg);
            if hoverable {
                i = i.group_color(k.accent_foreground);
            }
            content = content.child(i);
        }
        if let Some(l) = &self.label {
            let mut x = text(l.clone())
                .font(
                    ty::ui(ts)
                        .lh(ts * 1.25)
                        .weight(f.weight.unwrap_or(ty::NORMAL)),
                )
                .color(fg);
            if hoverable {
                x = x.group_color(k.accent_foreground);
            }
            content = content.child(x);
        }
        e = e.child(content);
        if hoverable {
            e = e.hover(k.accent).hover_group();
        }
        e = e
            .cursor(if self.disabled {
                Cursor::Default
            } else {
                f.cursor.unwrap_or(Cursor::Default)
            })
            .disabled(self.disabled);
        if let Some(l) = &self.label {
            e = e.label(l.clone());
        }
        if self.disabled {
            e = e.layer_opacity(0.5);
        }
        if let Some(tip) = self.tooltip {
            e = crate::tooltip::tooltip(e, tip);
        }
        match self.on_toggle {
            Some(act) if !self.disabled => act(e, !self.checked),
            _ => e,
        }
    }
}

type GroupAct = Box<dyn FnOnce(Vec<Toggle>, Vec<bool>, bool) -> Vec<Toggle>>;

/// gpui-kit's `ToggleGroup`: toggles in a row, 7 apart or joined (`segmented`) with only the
/// outer corners rounded. `on_press` gets every toggle's next state (`single`: only the
/// pressed one can be on).
pub struct ToggleGroup {
    key: String,
    size: KitSize,
    variant: ToggleVariant,
    disabled: bool,
    segmented: bool,
    single: bool,
    gap: Option<f32>,
    items: Vec<Toggle>,
    on_change: Option<GroupAct>,
}

impl ToggleGroup {
    pub fn new(key: impl Into<String>) -> Self {
        ToggleGroup {
            key: key.into(),
            size: KitSize::Medium,
            variant: ToggleVariant::Ghost,
            disabled: false,
            segmented: false,
            single: false,
            gap: None,
            items: Vec::new(),
            on_change: None,
        }
    }
    pub fn child(mut self, t: Toggle) -> Self {
        self.items.push(t);
        self
    }
    pub fn children(mut self, ts: impl IntoIterator<Item = Toggle>) -> Self {
        self.items.extend(ts);
        self
    }
    pub fn segmented(mut self) -> Self {
        self.segmented = true;
        self
    }
    pub fn single(mut self) -> Self {
        self.single = true;
        self
    }
    pub fn outline(mut self) -> Self {
        self.variant = ToggleVariant::Outline;
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// Gap between the toggles (gpui-kit: 7, none when segmented).
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = Some(g);
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S, Vec<bool>) + 'static) -> Self {
        self.on_change = Some(Box::new(
            move |items: Vec<Toggle>, checks: Vec<bool>, single: bool| {
                let f = std::rc::Rc::new(f);
                items
                    .into_iter()
                    .enumerate()
                    .map(|(ix, item)| {
                        let mut next = if single {
                            vec![false; checks.len()]
                        } else {
                            checks.clone()
                        };
                        next[ix] = !checks[ix];
                        let f = f.clone();
                        let mut item = item;
                        item.on_toggle = Some(Box::new(move |e: El, _| {
                            e.on_press(move |s: &mut S| f(s, next.clone()))
                        }));
                        item
                    })
                    .collect()
            },
        ));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let n = self.items.len();
        let checks: Vec<bool> = self.items.iter().map(|i| i.checked).collect();
        let mut items: Vec<Toggle> = self
            .items
            .into_iter()
            .enumerate()
            .map(|(ix, mut item)| {
                if self.segmented && n > 1 {
                    let first = ix == 0;
                    let last = ix == n - 1;
                    item.corners = [first, last, last, first];
                    item.edges = [true, true, true, first];
                }
                item.disabled |= self.disabled;
                item.size = self.size;
                item.variant = self.variant;
                item
            })
            .collect();
        if let Some(act) = self.on_change.filter(|_| !self.disabled) {
            items = act(items, checks, self.single);
        }
        let gap = self
            .gap
            .unwrap_or(if self.segmented { 0.0 } else { rems(0.5) });
        row()
            .key(self.key)
            .center_y()
            .gap(gap)
            .children(items.into_iter().map(|i| i.build(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, col};

    #[derive(Default)]
    struct S {
        presses: u32,
        disabled: bool,
        days: Vec<bool>,
    }

    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let t = Theme::dark();
        let b = primary_button("save", "Save").on_press(|s: &mut S| s.presses += 1);
        let days = ToggleGroup::new("days")
            .children((0..3).map(|i| {
                Toggle::new(format!("day{i}"))
                    .label("M")
                    .checked(s.days.get(i).copied().unwrap_or(false))
            }))
            .on_press(|s: &mut S, next: Vec<bool>| s.days = next);
        col()
            .child(disabled(b, s.disabled).build(t))
            .child(days.build(t))
    }

    fn press(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).expect(key);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_press_runs_the_handler_and_a_disabled_button_ignores_it() {
        let mut rt = Runtime::new(
            S::default(),
            view,
            (400.0, 200.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        press(&mut rt, "save");
        assert_eq!(rt.state.presses, 1);
        rt.update(|s, _| s.disabled = true);
        rt.frame(1.0 / 60.0);
        press(&mut rt, "save");
        assert_eq!(rt.state.presses, 1);
    }

    #[test]
    fn a_toggle_group_reports_the_next_states() {
        let mut rt = Runtime::new(
            S {
                days: vec![false, true, false],
                ..S::default()
            },
            view,
            (400.0, 200.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        press(&mut rt, "day2");
        assert_eq!(rt.state.days, vec![false, true, true]);
    }

    #[test]
    fn kit_sizes_are_in_rems() {
        assert_eq!(KitSize::Medium.icon_px(), 14.0);
        assert_eq!(KitSize::Large.icon_px(), 21.0);
    }
}
