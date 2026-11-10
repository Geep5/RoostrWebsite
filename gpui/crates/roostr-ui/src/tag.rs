//! Tags and badges: gpui-kit's `Tag` and `Badge` (as [`CountBadge`]), and Roostr's option pill
//! ([`TagPill`], [`tag`]) and property badge ([`Badge`]) from `roostr_core::ui`.

use rune::{Color, Cursor, El, Tx, el, row, text};

use crate::button::{Button, ButtonCustomVariant, KitSize};
use crate::color::brighten;
use crate::icons::{Glyph, glyph};
use crate::tags::{KitColor, TagPaint, badge_colors, tag_colors, unknown_tag_paint};
use crate::theme::{Theme, radius, rems, ty};
use crate::ty::Weights as _;

/// gpui-kit's `TagVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TagVariant {
    Primary,
    #[default]
    Secondary,
    Danger,
    Success,
    Warning,
    Info,
    Color(KitColor),
    Custom {
        color: Color,
        foreground: Color,
        border: Color,
    },
}

impl TagVariant {
    fn bg(self, t: &Theme) -> Color {
        let k = &t.kit;
        match self {
            Self::Primary => k.primary,
            Self::Secondary => k.secondary,
            Self::Danger => k.danger,
            Self::Success => k.success,
            Self::Warning => k.warning,
            Self::Info => k.info,
            Self::Color(c) => {
                if t.dark {
                    crate::button::opacity(c.scale(950), 0.5)
                } else {
                    c.scale(50)
                }
            }
            Self::Custom { color, .. } => color,
        }
    }
    fn border(self, t: &Theme) -> Color {
        let k = &t.kit;
        match self {
            Self::Primary => k.primary,
            Self::Secondary => k.border,
            Self::Danger => k.danger,
            Self::Success => k.success,
            Self::Warning => k.warning,
            Self::Info => k.info,
            Self::Color(c) => {
                if t.dark {
                    crate::button::opacity(c.scale(800), 0.5)
                } else {
                    c.scale(200)
                }
            }
            Self::Custom { border, .. } => border,
        }
    }
    fn fg(self, outline: bool, t: &Theme) -> Color {
        let k = &t.kit;
        let o = |a: Color, b: Color| if outline { a } else { b };
        match self {
            Self::Primary => o(k.primary, k.primary_foreground),
            Self::Secondary => o(k.muted_foreground, k.secondary_foreground),
            Self::Danger => o(k.danger, k.danger_foreground),
            Self::Success => o(k.success, k.success_foreground),
            Self::Warning => o(k.warning, k.warning_foreground),
            Self::Info => o(k.info, k.info_foreground),
            Self::Color(c) => {
                if t.dark {
                    c.scale(300)
                } else {
                    c.scale(600)
                }
            }
            Self::Custom { foreground, .. } => foreground,
        }
    }
}

/// gpui-kit's `Tag`: a 1px-bordered label (10.5px, line height 1.25; padding 3.5 × 8.75,
/// radius 6; Small/XSmall 8.75px, 1.75 × 5.25, radius 3; Large 12.25px) fading to 0.9 on hover.
/// The overrides take Roostr's per-use styling (`tag`, `Badge`).
pub struct Tag {
    variant: TagVariant,
    outline: bool,
    size: KitSize,
    radius: Option<f32>,
    label: Option<String>,
    icon: Option<(Glyph, f32, Color)>,
    children: Vec<El>,
    h: Option<f32>,
    line_height: Option<f32>,
    pad: [Option<f32>; 4],
    text_size: Option<f32>,
    weight: Option<f32>,
    gap: f32,
    border_w: f32,
    ellipsis: bool,
}

impl Default for Tag {
    fn default() -> Self {
        Self::new()
    }
}

impl Tag {
    pub fn new() -> Self {
        Tag {
            variant: TagVariant::Secondary,
            outline: false,
            size: KitSize::Medium,
            radius: None,
            label: None,
            icon: None,
            children: Vec::new(),
            h: None,
            line_height: None,
            pad: [None; 4],
            text_size: None,
            weight: None,
            gap: 0.0,
            border_w: 1.0,
            ellipsis: false,
        }
    }
    pub fn primary() -> Self {
        Self::new().variant(TagVariant::Primary)
    }
    pub fn secondary() -> Self {
        Self::new().variant(TagVariant::Secondary)
    }
    pub fn danger() -> Self {
        Self::new().variant(TagVariant::Danger)
    }
    pub fn success() -> Self {
        Self::new().variant(TagVariant::Success)
    }
    pub fn warning() -> Self {
        Self::new().variant(TagVariant::Warning)
    }
    pub fn info() -> Self {
        Self::new().variant(TagVariant::Info)
    }
    pub fn color(c: KitColor) -> Self {
        Self::new().variant(TagVariant::Color(c))
    }
    pub fn custom(color: Color, foreground: Color, border: Color) -> Self {
        Self::new().variant(TagVariant::Custom {
            color,
            foreground,
            border,
        })
    }
    pub fn variant(mut self, v: TagVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn rounded(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
    pub fn rounded_full(self) -> Self {
        self.rounded(rems(1.0))
    }
    pub fn label(mut self, s: impl Into<String>) -> Self {
        self.label = Some(s.into());
        self
    }
    /// A leading glyph at `size` in `color`.
    pub fn icon(mut self, g: Glyph, size: f32, color: Color) -> Self {
        self.icon = Some((g, size, color));
        self
    }
    pub fn child(mut self, e: El) -> Self {
        self.children.push(e);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.h = Some(h);
        self
    }
    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = Some(lh);
        self
    }
    pub fn px(mut self, p: f32) -> Self {
        self.pad[0] = Some(p);
        self.pad[2] = Some(p);
        self
    }
    pub fn py(mut self, p: f32) -> Self {
        self.pad[1] = Some(p);
        self.pad[3] = Some(p);
        self
    }
    pub fn pl(mut self, p: f32) -> Self {
        self.pad[0] = Some(p);
        self
    }
    pub fn pr(mut self, p: f32) -> Self {
        self.pad[2] = Some(p);
        self
    }
    pub fn text_size(mut self, s: f32) -> Self {
        self.text_size = Some(s);
        self
    }
    pub fn weight(mut self, w: f32) -> Self {
        self.weight = Some(w);
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = g;
        self
    }
    /// `.border_0()`.
    pub fn border_0(mut self) -> Self {
        self.border_w = 0.0;
        self
    }
    /// One line, ellipsis when squeezed.
    pub fn ellipsis(mut self) -> Self {
        self.ellipsis = true;
        self
    }

    fn body(self, t: &Theme) -> El {
        let v = self.variant;
        let bg = if self.outline {
            Color::WHITE.alpha(0.0)
        } else {
            v.bg(t)
        };
        let fg = v.fg(self.outline, t);
        let small = matches!(self.size, KitSize::XSmall | KitSize::Small);
        let ts = self.text_size.unwrap_or(match self.size {
            KitSize::XSmall | KitSize::Small => rems(0.625),
            KitSize::Large => rems(0.875),
            _ => rems(0.75),
        });
        let (px, py) = if small {
            (rems(0.375), rems(0.125))
        } else {
            (rems(0.625), rems(0.25))
        };
        let r = self.radius.unwrap_or(if small {
            radius::KIT / 2.0
        } else {
            radius::KIT
        });
        let lh = self.line_height.unwrap_or(ts * 1.25);
        let mut e = row()
            .center_y()
            .gap(self.gap)
            .pl(self.pad[0].unwrap_or(px))
            .pt(self.pad[1].unwrap_or(py))
            .pr(self.pad[2].unwrap_or(px))
            .pb(self.pad[3].unwrap_or(py))
            .radius(r)
            .fill(bg);
        if self.border_w > 0.0 {
            e = e.border(self.border_w, v.border(t)).border_layout();
        }
        if let Some(h) = self.h {
            e = e.h(h);
        }
        if let Some((g, s, c)) = self.icon {
            e = e.child(glyph(g, s).color(c));
        }
        if let Some(l) = self.label {
            let mut x = text(l)
                .font(ty::ui(ts).lh(lh).weight(self.weight.unwrap_or(ty::NORMAL)))
                .color(fg);
            if self.ellipsis {
                x = x.truncate().shrink();
            }
            e = e.child(x);
        }
        e.children(self.children)
    }

    /// The tag, fading to 0.9 while hovered.
    pub fn build(self, t: &Theme) -> El {
        el().hover_group()
            .child(self.body(t).group_opacity(1.0, 0.9))
    }
}

/// The kit [`Tag`] as Roostr draws an option (`roostr_core::ui::tag`): `paint`'s fill, 1px
/// border and text, radius 10, padding 0 6, one line of `height` (line height `height - 2`) at
/// `text_size`, ellipsis when squeezed.
pub fn tag(paint: TagPaint, height: f32, text_size: f32) -> Tag {
    Tag::custom(paint.bg, paint.fg, paint.border)
        .rounded(10.0)
        .h(height)
        .line_height(height - 2.0)
        .px(6.0)
        .py(0.0)
        .text_size(text_size)
        .ellipsis()
}

/// An option pill (Anytype tagItem, `roostr_core::ui::TagPill`): h20, padding 0 6, radius 10,
/// 12px, in its palette colour's look; an unknown colour is neutral with fg text.
pub struct TagPill {
    text: String,
    color: String,
}

impl TagPill {
    pub fn new(text: impl Into<String>, color: impl Into<String>) -> Self {
        TagPill {
            text: text.into(),
            color: color.into(),
        }
    }
    pub fn build(self, t: &Theme) -> El {
        let paint = tag_colors(t, &self.color).unwrap_or_else(|| unknown_tag_paint(t.fg));
        tag(paint, 20.0, 12.0).label(self.text).build(t)
    }
}

/// A property badge (`ObjectMeta .badge`, `roostr_core::ui::Badge`): h26, padding 0 10 0 6,
/// gap 6, 13/500, a leading 16px glyph in the badge's icon colour, radius 8, hover brightness
/// 1.18; pointer and tooltip-friendly when it has a press handler.
pub struct Badge {
    key: String,
    text: String,
    color: String,
    icon: Glyph,
    tooltip: Option<String>,
    button: Option<Box<dyn FnOnce(Button) -> Button>>,
}

impl Badge {
    /// `color` is a palette name ("" = neutral).
    pub fn new(
        key: impl Into<String>,
        text: impl Into<String>,
        color: impl Into<String>,
        icon: Glyph,
    ) -> Self {
        Badge {
            key: key.into(),
            text: text.into(),
            color: color.into(),
            icon,
            tooltip: None,
            button: None,
        }
    }
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.button = Some(Box::new(move |b: Button| b.on_press(f)));
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.button = Some(Box::new(move |b: Button| b.on_press_tx(f)));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let c = badge_colors(t, &self.color);
        let hover = brighten(c.bg, 1.18);
        let fill = ButtonCustomVariant::new(t)
            .color(c.bg)
            .foreground(c.fg)
            .hover(hover)
            .active(hover);
        let content = Tag::custom(Color::TRANSPARENT, c.fg, Color::TRANSPARENT)
            .rounded(8.0)
            .border_0()
            .gap(6.0)
            .pl(6.0)
            .pr(10.0)
            .py(0.0)
            .h(26.0)
            .text_size(13.0)
            .weight(ty::MEDIUM)
            .icon(self.icon, 16.0, c.icon)
            .label(self.text.clone())
            .body(t);
        let mut b = Button::new(self.key)
            .custom(fill)
            .radius(8.0)
            .accessibility_label(self.text)
            .h(26.0)
            .p(0.0)
            .bg(c.bg)
            .text_color(c.fg)
            .child(content);
        if let Some(tip) = self.tooltip {
            b = b.tooltip(tip);
        }
        if let Some(attach) = self.button {
            b = attach(b).cursor(Cursor::Pointer);
        }
        b.build(t)
    }
}

/// gpui-kit's `Badge` (a counter, dot or icon on a child) as `CountBadge`: a red (or `color`)
/// pill with white 10px text at the child's top-right corner (Medium: 3 up, 3 per digit out);
/// `count` 0 hides it, above `max` (99) shows "99+". A changed count rolls its digits
/// ([`motion::rolling_text`]); `key` names the badge (unique in the window).
pub struct CountBadge {
    key: String,
    child: El,
    count: usize,
    max: usize,
    dot: bool,
    icon: Option<Glyph>,
    color: Option<Color>,
    size: KitSize,
}

impl CountBadge {
    pub fn new(key: impl Into<String>, child: El) -> Self {
        CountBadge {
            key: key.into(),
            child,
            count: 0,
            max: 99,
            dot: false,
            icon: None,
            color: None,
            size: KitSize::Medium,
        }
    }
    pub fn count(mut self, n: usize) -> Self {
        self.count = n;
        self
    }
    pub fn max(mut self, n: usize) -> Self {
        self.max = n;
        self
    }
    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = Some(g);
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let fill = self.color.unwrap_or(t.kit.red);
        let (side, text_size) = match self.size {
            KitSize::Large => (24.0, 14.0),
            KitSize::Medium | KitSize::Px(_) => (16.0, 10.0),
            _ => (10.0, 8.0),
        };
        let pill = el().center().radius(f32::MAX / 4.0).fill(fill);
        let badge = if let Some(g) = self.icon {
            Some(
                pill.pin(None, None, Some(0.0), Some(0.0))
                    .size(side, side)
                    .border(1.0, t.kit.background)
                    .child(glyph(g, text_size).color(Color::WHITE)),
            )
        } else if self.dot {
            Some(pill.pin(None, Some(0.0), Some(0.0), None).size(6.0, 6.0))
        } else if self.count > 0 {
            let n = if self.count > self.max {
                format!("{}+", self.max)
            } else {
                self.count.to_string()
            };
            let len = n.len() as f32;
            let (top, right) = match self.size {
                KitSize::Large => (2.0, -len),
                KitSize::Medium | KitSize::Px(_) => (-3.0, -3.0 * len),
                _ => (-4.0, -4.0 * len),
            };
            let number = crate::motion::rolling_text(
                t,
                format!("{}-count", self.key),
                n,
                self.count.min(self.max + 1) as f64,
                ty::ui(10.0).lh(10.0),
                Color::WHITE,
            );
            Some(
                pill.pin(None, Some(top), Some(right), None)
                    .padding(rems(0.125))
                    .min_w(rems(0.875))
                    .child(number),
            )
        } else {
            None
        };
        el().child(self.child).children(badge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, col};

    #[derive(Default)]
    struct S {
        opened: u32,
    }

    fn view(_: &S, _: &mut rune::Cx<S>) -> El {
        let t = Theme::dark();
        col()
            .gap(8.0)
            .child(
                Badge::new("status", "Done", "green", Glyph::BadgeCheck)
                    .on_press(|s: &mut S| s.opened += 1)
                    .build(t),
            )
            .child(Badge::new("plain", "Note", "", Glyph::BadgeDot).build(t))
    }

    fn press(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).expect(key);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_clickable_badge_presses_and_a_plain_one_does_not() {
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
        press(&mut rt, "status");
        assert_eq!(rt.state.opened, 1);
        press(&mut rt, "plain");
        assert_eq!(rt.state.opened, 1);
        let rect = |k: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(k))
                .unwrap()
                .rect
        };
        assert_eq!(rect("status")[3], 26.0);
        assert_eq!(rect("plain")[3], 26.0);
    }
}
