//! Display components: gpui-kit's `Label`, `Link`, `Separator`, `Kbd`, `Avatar`, `Alert`,
//! `Empty` (and its header parts), `Spinner`, `Progress`, `ProgressCircle`, `Skeleton` and
//! `ShimmerText`, on Rune.
//!
//! Each builder ends in `.build(t) -> El`; the returned element takes Rune's own layout calls
//! (`.px(..)`, `.w_full()`, `.h(..)`, `.radius(..)`) for the per-use overrides the screens chain
//! onto the gpui-kit element (a later call replaces the builder's value).

use rune::{Color, Cursor, El, LoopSpec, Pixels, Shape, Span, TextStyle, Tx, col, el, row, text};

use crate::button::{KitSize, opacity};
use crate::color::{from_srgb, to_srgb};
use crate::icons::{Glyph, glyph};
use crate::overlay::Acts;
use crate::theme::{Theme, radius, rems, ty};
use crate::ty::Weights as _;

// ------------------------------------------------------------------ oklab

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_oklab(c: Color) -> [f32; 3] {
    let [r, g, b] = to_srgb(c).map(to_linear);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn from_oklab([ll, a, b]: [f32; 3], alpha: f32) -> Color {
    let l = (ll + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (ll - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (ll - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let rgb = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ];
    from_srgb(rgb.map(|c| from_linear(c).clamp(0.0, 1.0)), alpha)
}

/// gpui-kit's `Colorize::mix_oklab`: `a` at `factor`, `b` at the rest, premultiplied in Oklab.
pub(crate) fn mix_oklab(a: Color, b: Color, factor: f32) -> Color {
    let f = factor.clamp(0.0, 1.0);
    let alpha = a.a * f + b.a * (1.0 - f);
    if alpha == 0.0 {
        return Color::TRANSPARENT;
    }
    let (x, y) = (to_oklab(a), to_oklab(b));
    let m = |i: usize| (x[i] * a.a * f + y[i] * b.a * (1.0 - f)) / alpha;
    from_oklab([m(0), m(1), m(2)], alpha)
}

/// gpui-kit's `oklch(l, c, hue°)`.
pub(crate) fn oklch(l: f32, c: f32, hue: f32) -> Color {
    let h = hue.to_radians();
    from_oklab([l, c * h.cos(), c * h.sin()], 1.0)
}

// ------------------------------------------------------------------ Label

/// gpui-kit's `Label`: one text (14px, line height 1.25 rem, kit foreground), an optional
/// muted secondary text after a space, masking (•) and highlighted matches (kit blue).
pub struct Label {
    text: String,
    secondary: Option<String>,
    masked: bool,
    highlight: Option<(String, bool)>,
    size: f32,
    weight: f32,
    color: Option<Color>,
    line_height: f32,
    truncate: bool,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Label {
            text: text.into(),
            secondary: None,
            masked: false,
            highlight: None,
            size: rems(1.0),
            weight: ty::NORMAL,
            color: None,
            line_height: rems(1.25),
            truncate: false,
        }
    }
    pub fn secondary(mut self, s: impl Into<String>) -> Self {
        self.secondary = Some(s.into());
        self
    }
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }
    /// Highlight every case-insensitive occurrence of `m`.
    pub fn highlights(mut self, m: impl Into<String>) -> Self {
        self.highlight = Some((m.into(), false));
        self
    }
    /// Highlight `m` only when the text starts with it.
    pub fn highlights_prefix(mut self, m: impl Into<String>) -> Self {
        self.highlight = Some((m.into(), true));
        self
    }
    pub fn text_size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = lh;
        self
    }
    /// One line, ellipsis when squeezed (`.overflow_hidden().text_ellipsis()`).
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }

    fn ranges(&self, full: &str) -> Vec<(usize, usize)> {
        let Some((m, prefix)) = &self.highlight else {
            return Vec::new();
        };
        if m.is_empty() {
            return Vec::new();
        }
        let (lower, needle) = (full.to_lowercase(), m.to_lowercase());
        if lower.len() != full.len() {
            return Vec::new();
        }
        if *prefix {
            return if lower.starts_with(&needle) {
                vec![(0, m.len())]
            } else {
                Vec::new()
            };
        }
        lower
            .match_indices(&needle)
            .map(|(i, _)| (i, i + m.len()))
            .collect()
    }

    pub fn build(self, t: &Theme) -> El {
        let full = match &self.secondary {
            Some(s) => format!("{} {}", self.text, s),
            None => self.text.clone(),
        };
        let mut spans = Vec::new();
        let shown = if self.masked {
            "•".repeat(full.chars().count())
        } else {
            if self.secondary.is_some() {
                spans.push(Span::new(self.text.len(), full.len()).color(t.kit.muted_foreground));
            }
            spans.extend(
                self.ranges(&full)
                    .into_iter()
                    .map(|(a, b)| Span::new(a, b).color(t.kit.blue)),
            );
            full
        };
        let mut e = text(shown)
            .font(ty::ui(self.size).lh(self.line_height).weight(self.weight))
            .color(self.color.unwrap_or(t.kit.foreground));
        if !spans.is_empty() {
            e = e.spans(spans);
        }
        if self.truncate {
            e = e.truncate().shrink();
        }
        e
    }
}

// ------------------------------------------------------------------ Link

/// gpui-kit's `Link`: kit link colour, underlined at half strength (full on hover/press), 0.8
/// on hover, 0.6 pressed, pointer; opens `href` and/or runs its handler on a press.
pub struct Link {
    key: String,
    label: String,
    href: Option<String>,
    disabled: bool,
    size: f32,
    weight: f32,
    color: Option<Color>,
    underline: Option<Color>,
    acts: Acts,
}

impl Link {
    pub fn new(key: impl Into<String>) -> Self {
        Link {
            key: key.into(),
            label: String::new(),
            href: None,
            disabled: false,
            size: rems(1.0),
            weight: ty::NORMAL,
            color: None,
            underline: None,
            acts: Acts::default(),
        }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = l.into();
        self
    }
    pub fn href(mut self, url: impl Into<String>) -> Self {
        self.href = Some(url.into());
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn text_size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }
    /// Text colour (`.text_color(..)`); hover/press fade it like the kit link colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Underline colour at rest (`.text_decoration_color(..)`); transparent hides it.
    pub fn underline(mut self, c: Color) -> Self {
        self.underline = Some(c);
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let ink = self.color.unwrap_or(t.kit.link);
        let line = self.underline.unwrap_or(opacity(ink, 0.5));
        let n = self.label.len();
        let label = text(self.label.clone())
            .font(ty::ui(self.size).weight(self.weight))
            .color(ink)
            .group_color(opacity(ink, 0.8))
            .spans(vec![Span::new(0, n).underline(line)]);
        let mut e = row()
            .key(self.key)
            .role("link")
            .label(self.label)
            .hover_group()
            .cursor(Cursor::Pointer)
            .child(label);
        if let (Some(url), false) = (self.href, self.disabled) {
            // Rune opens it on a press (`Tx::open_url`).
            e = e.href(url);
        }
        self.acts.apply(e, !self.disabled)
    }
}

// ------------------------------------------------------------------ Separator

/// gpui-kit's `Separator`: a 1px kit border line across (or down) its box, solid or dashed
/// (4 on, 2 off), with an optional centred label (10.5px muted on the kit background, 7 × 3.5
/// padding).
pub struct Separator {
    vertical: bool,
    dashed: bool,
    label: Option<String>,
    color: Option<Color>,
}

impl Separator {
    pub fn horizontal() -> Self {
        Separator {
            vertical: false,
            dashed: false,
            label: None,
            color: None,
        }
    }
    pub fn vertical() -> Self {
        Separator {
            vertical: true,
            ..Separator::horizontal()
        }
    }
    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let c = self.color.unwrap_or(t.kit.border);
        let line = if self.vertical {
            el().w(1.0).h_full()
        } else {
            el().h(1.0).w_full()
        };
        let line = if self.dashed {
            let side = if self.vertical {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [1.0, 0.0, 0.0, 0.0]
            };
            line.border_sides(side, c).border_dash(4.0, 2.0)
        } else {
            line.fill(c)
        };
        // Out of flow and centred across the box, as gpui-kit's absolute line in a centring
        // flex box.
        let line = el()
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .center()
            .child(line);
        let mut e = if self.vertical {
            col().h_full().w(1.0)
        } else {
            row().w_full()
        }
        .center();
        if !self.vertical && self.label.is_none() {
            e = e.h(1.0);
        }
        e = e.child(line);
        if let Some(l) = self.label {
            e = e.child(
                el().px(rems(0.5))
                    .py(rems(0.25))
                    .fill(t.kit.background)
                    .child(
                        text(l)
                            .font(ty::ui(rems(0.75)))
                            .color(t.kit.muted_foreground),
                    ),
            );
        }
        e
    }
}

// ------------------------------------------------------------------ Kbd

/// gpui-kit's `Kbd`: a keystroke formatted for this platform (`⌘K`; `Ctrl+K` elsewhere) on the
/// kit muted fill, 10.5px muted text, padding 1.75 × 3.5, min width 17.5, radius 3; `outline`
/// borders it on the kit background; `appearance(false)` is the bare text.
pub struct Kbd {
    keys: String,
    appearance: bool,
    outline: bool,
}

impl Kbd {
    /// A GPUI keystroke string: modifiers then the key, `-` apart (`"cmd-shift-k"`,
    /// `"ctrl-enter"`).
    pub fn new(keystroke: &str) -> Self {
        let mut parts: Vec<&str> = keystroke.split('-').collect();
        let key = parts.pop().unwrap_or("");
        let (key, parts) = if key.is_empty() && !parts.is_empty() {
            parts.pop();
            ("-", parts)
        } else {
            (key, parts)
        };
        let has = |m: &[&str]| parts.iter().any(|p| m.contains(p));
        let keys = crate::tooltip::format_keys(
            has(&["ctrl", "control"]),
            has(&["alt", "option"]),
            has(&["shift"]),
            has(&["cmd", "super", "win", "platform"]),
            key,
        );
        Kbd {
            keys,
            appearance: true,
            outline: false,
        }
    }
    pub fn appearance(mut self, on: bool) -> Self {
        self.appearance = on;
        self
    }
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }
    /// The formatted keys (gpui-kit's `Kbd::format`).
    pub fn keys(&self) -> &str {
        &self.keys
    }

    pub fn build(self, t: &Theme) -> El {
        let k = &t.kit;
        let size = rems(0.75);
        if !self.appearance {
            return text(self.keys).font(ty::ui(size)).color(k.foreground);
        }
        let mut e = row()
            .center()
            .py(rems(0.125))
            .px(rems(0.25))
            .min_w(rems(1.25))
            .radius(radius::KIT * 0.5)
            .fill(k.muted)
            .child(
                text(self.keys)
                    .font(ty::ui(size).lh(size))
                    .color(k.muted_foreground),
            );
        if self.outline {
            e = e.fill(k.background).border(1.0, k.border).border_layout();
        }
        e
    }
}

// ------------------------------------------------------------------ Avatar

/// gpui-kit's `Avatar`: a round picture (16 / 24 / 48 / 80 for XSmall … Large), a name's
/// initials on its identity colour, or the placeholder icon (User) at 60% of the size in the
/// kit background colour on the kit secondary fill, inside a 1px border.
pub struct Avatar {
    src: Option<Pixels>,
    name: Option<String>,
    placeholder: Glyph,
    placeholder_size: Option<f32>,
    size: KitSize,
    radius: Option<f32>,
}

impl Default for Avatar {
    fn default() -> Self {
        Self::new()
    }
}

impl Avatar {
    pub fn new() -> Self {
        Avatar {
            src: None,
            name: None,
            placeholder: Glyph::User,
            placeholder_size: None,
            size: KitSize::Medium,
            radius: None,
        }
    }
    /// The picture, decoded (`rune::ImageCache`); Rune has no HTTP client, so a remote picture
    /// is fetched by the caller.
    pub fn src(mut self, pixels: Pixels) -> Self {
        self.src = Some(pixels);
        self
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn placeholder(mut self, g: Glyph) -> Self {
        self.placeholder = g;
        self
    }
    /// The placeholder icon's size (gpui-kit: `Icon::size`).
    pub fn placeholder_size(mut self, s: f32) -> Self {
        self.placeholder_size = Some(s);
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    /// Corner radius (gpui-kit: fully round).
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }

    /// `extract_text_initials`: the first letters of the first two words, or the first two
    /// characters of a one-word name, uppercased.
    pub fn initials(name: &str) -> String {
        let firsts: String = name
            .split(' ')
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect();
        let r = if firsts.len() == 1 {
            name.chars().take(2).collect()
        } else {
            firsts
        };
        r.to_uppercase()
    }

    pub fn build(self, t: &Theme) -> El {
        let px = match self.size {
            KitSize::Large => 80.0,
            KitSize::Medium => 48.0,
            KitSize::Small => 24.0,
            KitSize::XSmall => 16.0,
            KitSize::Px(s) => s,
        };
        let r = self.radius.unwrap_or(px / 2.0);
        let initials = self.name.as_deref().map(Avatar::initials);
        let identity = initials.as_deref().map(|s| identity(s, t.dark));
        let border = match (&identity, &self.src) {
            (Some(i), None) => i[2],
            _ => t.kit.border,
        };
        let mut e = el()
            .size(px, px)
            .radius(r)
            .clip_rounded(r)
            .fill(t.kit.secondary)
            .border(1.0, border)
            .center();
        if let Some(name) = &self.name {
            e = e.label(name.clone());
        }
        e = match (self.src, initials, identity) {
            (Some(p), _, _) => e.child(el().w_full().h_full().radius(r).pixels(&p)),
            (None, Some(s), Some(i)) => {
                let font = match self.size {
                    KitSize::Large => ty::ui(rems(1.875)).semibold(),
                    KitSize::Medium => ty::ui(rems(0.875)),
                    KitSize::Small => ty::ui(rems(0.75)),
                    KitSize::XSmall => ty::ui(rems(0.65)),
                    KitSize::Px(_) => ty::ui(rems(1.0)),
                };
                e.fill(i[0]).child(text(s).font(font).color(i[1]))
            }
            _ => e.child(
                glyph(self.placeholder, self.placeholder_size.unwrap_or(px * 0.6))
                    .color(t.kit.background),
            ),
        };
        e
    }
}

/// gpui-kit's `IdentityColor`: one of 12 hues from the initials' hash → fill, text, border.
fn identity(initials: &str, dark: bool) -> [Color; 3] {
    // FNV-1a: gpui hashed with FxHash; any stable hash spreads names over the 12 hues.
    let h = initials.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    let hue = (h % 12) as f32 * 30.0;
    if dark {
        [
            oklch(0.30, 0.05, hue),
            oklch(0.82, 0.11, hue),
            oklch(0.36, 0.06, hue),
        ]
    } else {
        [
            oklch(0.97, 0.032, hue),
            oklch(0.50, 0.145, hue),
            oklch(0.89, 0.05, hue),
        ]
    }
}

// ------------------------------------------------------------------ Alert

/// gpui-kit's `AlertVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertVariant {
    #[default]
    Default,
    Info,
    Success,
    Warning,
    Error,
}

/// gpui-kit's `Alert`: icon, optional semibold title and message in the variant colour on a 4%
/// tint with a 30% border (radius 6, padding 10 × 16, gap 12, 12.25px); `banner` drops the
/// radius and title and centres the row; a close button when `on_close` is set.
pub struct Alert {
    key: String,
    variant: AlertVariant,
    icon: Glyph,
    title: Option<String>,
    message: String,
    size: KitSize,
    banner: bool,
    close: Acts,
}

impl Alert {
    pub fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Alert {
            key: key.into(),
            variant: AlertVariant::Default,
            icon: Glyph::Info,
            title: None,
            message: message.into(),
            size: KitSize::Medium,
            banner: false,
            close: Acts::default(),
        }
    }
    pub fn info(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(key, message)
            .variant(AlertVariant::Info)
            .icon(Glyph::Info)
    }
    pub fn success(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(key, message)
            .variant(AlertVariant::Success)
            .icon(Glyph::CircleCheck)
    }
    pub fn warning(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(key, message)
            .variant(AlertVariant::Warning)
            .icon(Glyph::TriangleAlert)
    }
    pub fn error(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(key, message)
            .variant(AlertVariant::Error)
            .icon(Glyph::CircleX)
    }
    pub fn variant(mut self, v: AlertVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = g;
        self
    }
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s;
        self
    }
    pub fn banner(mut self) -> Self {
        self.banner = true;
        self
    }
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.close.press(f);
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let k = &t.kit;
        let tint = |c: Color, f: f32| opacity(c, f);
        let (fg, bg, border) = match self.variant {
            AlertVariant::Default => (k.foreground, k.background, k.border),
            AlertVariant::Info => (k.info, tint(k.info, 0.04), tint(k.info, 0.3)),
            AlertVariant::Success => (k.success, tint(k.success, 0.04), tint(k.success, 0.3)),
            AlertVariant::Warning => (k.warning, tint(k.warning, 0.04), tint(k.warning, 0.3)),
            AlertVariant::Error => (k.danger, tint(k.danger, 0.04), tint(k.danger, 0.3)),
        };
        let (r, px, py, gap) = match self.size {
            KitSize::XSmall => (radius::KIT, 12.0, 6.0, 6.0),
            KitSize::Small => (radius::KIT, 12.0, 8.0, 6.0),
            KitSize::Large => (radius::KIT_LG, 20.0, 14.0, 12.0),
            _ => (radius::KIT, 16.0, 10.0, 12.0),
        };
        let ts = rems(0.875);
        let font = ty::ui(ts);
        let mut body = col().grow(1.0).shrink();
        if let (false, Some(title)) = (self.banner, &self.title) {
            body = body.child(
                text(title.clone())
                    .font(font.semibold())
                    .color(fg)
                    .truncate(),
            );
        }
        body = body.child(text(self.message.clone()).font(font).color(fg).wrap());
        let mut icon = glyph(self.icon, ts).color(fg);
        if !self.banner {
            icon = icon.mt(5.0);
        }
        let mut lead = row().grow(1.0).shrink().gap(gap).child(icon).child(body);
        lead = if self.banner {
            lead.center_y()
        } else {
            lead.items_start()
        };
        let mut e = row()
            .key(self.key.clone())
            .role("alert")
            .w_full()
            .fill(bg)
            .border(1.0, border)
            .border_layout()
            .px(px)
            .py(py)
            .gap(gap)
            .justify_between()
            .child(lead);
        e = if self.banner {
            e.center_y()
        } else {
            e.radius(r).items_start()
        };
        if !self.close.is_empty() {
            let size = self.size.max_medium().icon_px();
            let close = row()
                .key(format!("{}-close", self.key))
                .role("button")
                .label("Close")
                .padding(rems(0.125))
                .radius(radius::KIT)
                .hover(opacity(bg, 0.8))
                .press(opacity(bg, 0.9))
                .child(glyph(Glyph::Close, size).color(fg));
            e = e.child(self.close.apply(close, true));
        }
        e
    }
}

impl KitSize {
    /// gpui-kit's `size.max(Size::Medium)`: the larger of this size and Medium.
    pub fn max_medium(self) -> KitSize {
        match self {
            KitSize::Large => KitSize::Large,
            KitSize::Px(p) if p > rems(1.0) => self,
            _ => KitSize::Medium,
        }
    }
}

// ------------------------------------------------------------------ Empty

/// gpui-kit's `Empty`: a centred column (gap 14, padding 21, radius 12, kit foreground) of an
/// [`EmptyHeader`], [`EmptyContent`] and children. Roostr's "nothing here" notes override it
/// on the returned element (`.items_start().gap(0.0).padding(0.0).px(8.0)`) and add a
/// [`Empty::text`] line in their size and colour.
pub struct Empty {
    header: Option<EmptyHeader>,
    content: Option<EmptyContent>,
    children: Vec<El>,
    texts: Vec<(String, f32, Option<Color>)>,
}

impl Default for Empty {
    fn default() -> Self {
        Self::new()
    }
}

impl Empty {
    pub fn new() -> Self {
        Empty {
            header: None,
            content: None,
            children: Vec::new(),
            texts: Vec::new(),
        }
    }
    pub fn header(mut self, h: EmptyHeader) -> Self {
        self.header = Some(h);
        self
    }
    pub fn content(mut self, c: EmptyContent) -> Self {
        self.content = Some(c);
        self
    }
    pub fn child(mut self, e: El) -> Self {
        self.children.push(e);
        self
    }
    /// A line of text at `size` (kit foreground unless `color`), as a string child of the kit's
    /// `Empty` took the element's text style.
    pub fn text(mut self, s: impl Into<String>, size: f32, color: Option<Color>) -> Self {
        self.texts.push((s.into(), size, color));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let mut e = col()
            .w_full()
            .center()
            .gap(rems(1.0))
            .padding(rems(1.5))
            .radius(radius::KIT * 2.0)
            .children(self.header.map(|h| h.build(t)))
            .children(self.content.map(|c| c.build()))
            .children(self.children);
        for (s, size, c) in self.texts {
            e = e.child(
                text(s)
                    .font(ty::ui(size))
                    .color(c.unwrap_or(t.kit.foreground))
                    .wrap(),
            );
        }
        e
    }
}

/// gpui-kit's `EmptyHeader`: media, title and description, centred, gap 7, max width 24 rem.
#[derive(Default)]
pub struct EmptyHeader {
    media: Option<EmptyMedia>,
    title: Option<String>,
    description: Option<String>,
}

impl EmptyHeader {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn media(mut self, m: EmptyMedia) -> Self {
        self.media = Some(m);
        self
    }
    /// gpui-kit's `EmptyTitle`: 12.25px medium.
    pub fn title(mut self, s: impl Into<String>) -> Self {
        self.title = Some(s.into());
        self
    }
    /// gpui-kit's `EmptyDescription`: 12.25px, line height 1.625, kit muted foreground.
    pub fn description(mut self, s: impl Into<String>) -> Self {
        self.description = Some(s.into());
        self
    }
    fn build(self, t: &Theme) -> El {
        let ts = rems(0.875);
        let mut e = col()
            .w_full()
            .max_w(rems(24.0))
            .items_center()
            .gap(rems(0.5));
        if let Some(m) = self.media {
            e = e.child(m.build(t));
        }
        if let Some(s) = self.title {
            e = e.child(
                text(s)
                    .font(ty::ui(ts).medium())
                    .color(t.kit.foreground)
                    .wrap(),
            );
        }
        if let Some(s) = self.description {
            e = e.child(
                text(s)
                    .font(ty::ui(ts).lh(ts * 1.625))
                    .color(t.kit.muted_foreground)
                    .wrap(),
            );
        }
        e
    }
}

/// gpui-kit's `EmptyMedia`: a glyph; the `Icon` variant puts it on a 28px kit-muted tile
/// (radius 10).
pub struct EmptyMedia {
    glyph: Glyph,
    tile: bool,
}

impl EmptyMedia {
    pub fn new(g: Glyph) -> Self {
        EmptyMedia {
            glyph: g,
            tile: false,
        }
    }
    pub fn icon(g: Glyph) -> Self {
        EmptyMedia {
            glyph: g,
            tile: true,
        }
    }
    fn build(self, t: &Theme) -> El {
        let mut e = col().center().mb(rems(0.5));
        if self.tile {
            e = e
                .size(rems(2.0), rems(2.0))
                .radius(radius::KIT_LG)
                .fill(t.kit.muted);
        }
        let size = if self.tile { rems(1.0) } else { rems(2.0) };
        e.child(glyph(self.glyph, size).color(t.kit.foreground))
    }
}

/// gpui-kit's `EmptyContent`: actions under the header, centred, gap 8.75.
#[derive(Default)]
pub struct EmptyContent {
    children: Vec<El>,
}

impl EmptyContent {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn child(mut self, e: El) -> Self {
        self.children.push(e);
        self
    }
    fn build(self) -> El {
        col()
            .w_full()
            .max_w(rems(24.0))
            .items_center()
            .gap(rems(0.625))
            .children(self.children)
    }
}

// ------------------------------------------------------------------ Spinner

/// gpui-kit's `Spinner`: the Loader icon turning once every 0.8 s (14px at Medium).
pub struct Spinner {
    size: f32,
    color: Option<Color>,
    icon: Glyph,
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Spinner {
    pub fn new() -> Self {
        Spinner {
            size: KitSize::Medium.icon_px(),
            color: None,
            icon: Glyph::Loader,
        }
    }
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = s.icon_px();
        self
    }
    pub fn px(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn icon(mut self, g: Glyph) -> Self {
        self.icon = g;
        self
    }
    pub fn build(self, t: &Theme) -> El {
        glyph(self.icon, self.size)
            .color(self.color.unwrap_or(t.kit.foreground))
            .role("progressbar")
            .label("Loading")
            .looping(LoopSpec::rotation(0.8))
    }
}

// ------------------------------------------------------------------ Progress

/// gpui-kit's `Progress`: a pill track (4 / 6 / 8 / 10 high) in the bar colour at 20% under the
/// filled part; `loading` sweeps a segment instead.
pub struct Progress {
    key: String,
    value: f32,
    color: Option<Color>,
    size: KitSize,
    loading: bool,
}

impl Progress {
    pub fn new(key: impl Into<String>) -> Self {
        Progress {
            key: key.into(),
            value: 0.0,
            color: None,
            size: KitSize::Medium,
            loading: false,
        }
    }
    /// 0 … 100.
    pub fn value(mut self, v: f32) -> Self {
        self.value = v.clamp(0.0, 100.0);
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
    pub fn loading(mut self, l: bool) -> Self {
        self.loading = l;
        self
    }
    pub fn build(self, t: &Theme) -> El {
        let c = self.color.unwrap_or(t.kit.progress_bar);
        let (h, r) = match self.size {
            KitSize::XSmall => (4.0, 2.0),
            KitSize::Small => (6.0, 3.0),
            KitSize::Medium => (8.0, 4.0),
            KitSize::Large => (10.0, 5.0),
            KitSize::Px(s) => (s, s / 2.0),
        };
        let bar = if self.loading {
            el().absolute(0.0, 0.0)
                .w_frac(0.35)
                .h_full()
                .radius(r)
                .fill(c)
                .looping(LoopSpec::slide(1.0, [-1.0, 0.0], [2.86, 0.0]))
        } else {
            el().absolute(0.0, 0.0)
                .w_frac(self.value / 100.0)
                .h_full()
                .radius(r)
                .fill(c)
        };
        el().key(self.key)
            .role("progressbar")
            .range_value(f64::from(self.value), 0.0, 100.0)
            .w_full()
            .h(h)
            .radius(r)
            .clip_rounded(r)
            .fill(opacity(c, 0.2))
            .child(bar)
    }
}

/// gpui-kit's `ProgressCircle`: a ring (stroke 15% of the size, at most 5) in the bar colour at
/// 20% with the done part from 12 o'clock clockwise; `loading` turns a half arc.
pub struct ProgressCircle {
    key: String,
    value: f32,
    color: Option<Color>,
    size: f32,
    loading: bool,
}

impl ProgressCircle {
    pub fn new(key: impl Into<String>) -> Self {
        ProgressCircle {
            key: key.into(),
            value: 0.0,
            color: None,
            size: rems(1.0),
            loading: false,
        }
    }
    pub fn value(mut self, v: f32) -> Self {
        self.value = v.clamp(0.0, 100.0);
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// gpui-kit sizes: XSmall 7, Small 10.5, Medium 14, Large 17.5, `Px(s)` 0.75 s.
    pub fn size(mut self, s: KitSize) -> Self {
        self.size = match s {
            KitSize::XSmall => rems(0.5),
            KitSize::Small => rems(0.75),
            KitSize::Medium => rems(1.0),
            KitSize::Large => rems(1.25),
            KitSize::Px(p) => p * 0.75,
        };
        self
    }
    /// A side in points (`.size(px(..))` on the element).
    pub fn px(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn loading(mut self, l: bool) -> Self {
        self.loading = l;
        self
    }

    fn arc(size: f32, from: f32, to: f32, color: Color) -> Shape {
        let sw = (size * 0.15).min(5.0);
        let r = (size - sw) / 2.0;
        let c = size / 2.0;
        let steps = ((to - from) * 64.0).ceil().max(2.0) as usize;
        let pts: Vec<[f32; 2]> = (0..=steps)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / steps as f32) * std::f32::consts::TAU;
                [c + r * a.sin(), c - r * a.cos()]
            })
            .collect();
        Shape::new().stroke(pts, sw, color)
    }

    pub fn build(self, t: &Theme) -> El {
        let c = self.color.unwrap_or(t.kit.progress_bar);
        let s = self.size;
        let track = rune::canvas(Self::arc(s, 0.0, 1.0, opacity(c, 0.2)).view_box(s, s))
            .absolute(0.0, 0.0)
            .size(s, s);
        let mut e = el()
            .key(self.key)
            .role("progressbar")
            .size(s, s)
            .child(track);
        if self.loading {
            e = e.child(
                rune::canvas(Self::arc(s, 0.0, 0.5, c).view_box(s, s))
                    .absolute(0.0, 0.0)
                    .size(s, s)
                    .looping(LoopSpec::rotation(1.0)),
            );
        } else {
            e = e.range_value(f64::from(self.value), 0.0, 100.0);
            if self.value > 0.0 {
                e = e.child(
                    rune::canvas(Self::arc(s, 0.0, self.value / 100.0, c).view_box(s, s))
                        .absolute(0.0, 0.0)
                        .size(s, s),
                );
            }
        }
        e
    }
}

// ------------------------------------------------------------------ Skeleton

/// gpui-kit's `Skeleton`: a full-width 14px block in the kit skeleton colour (half strength
/// when `secondary`) pulsing 1 → 0.5 → 1 every 2 s. Set its size and radius on the element.
pub struct Skeleton {
    secondary: bool,
}

impl Default for Skeleton {
    fn default() -> Self {
        Self::new()
    }
}

impl Skeleton {
    pub fn new() -> Self {
        Skeleton { secondary: false }
    }
    pub fn secondary(mut self) -> Self {
        self.secondary = true;
        self
    }
    pub fn build(self, t: &Theme) -> El {
        let c = if self.secondary {
            opacity(t.kit.skeleton, 0.5)
        } else {
            t.kit.skeleton
        };
        el().w_full()
            .h(rems(1.0))
            .fill(c)
            .looping(LoopSpec::opacity(2.0, 0.0, 1.0, 0.5))
    }
}

// ------------------------------------------------------------------ ShimmerText

/// gpui-kit's `ShimmerText`: a line whose glyphs a soft highlight band (30% of the width either
/// side of its centre) sweeps across every 2 s. The highlight is the text colour mixed 20%
/// toward the kit foreground (dark) or background (light), peaking at 60% / 75%.
pub struct ShimmerText {
    key: String,
    text: String,
    font: TextStyle,
    color: Option<Color>,
    highlight: Option<Color>,
    period: f64,
    spread: f32,
    reverse: bool,
}

impl ShimmerText {
    pub fn new(key: impl Into<String>, text: impl Into<String>) -> Self {
        ShimmerText {
            key: key.into(),
            text: text.into(),
            font: ty::ui(rems(1.0)),
            color: None,
            highlight: None,
            period: 2.0,
            spread: 0.3,
            reverse: false,
        }
    }
    pub fn font(mut self, f: TextStyle) -> Self {
        self.font = f;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn highlight_color(mut self, c: Color) -> Self {
        self.highlight = Some(c);
        self
    }
    /// One sweep in seconds.
    pub fn duration(mut self, s: f64) -> Self {
        self.period = s.max(0.001);
        self
    }
    /// Half the band's width as a fraction of the text's (0.05 … 1).
    pub fn spread(mut self, f: f32) -> Self {
        self.spread = f.clamp(0.05, 1.0);
        self
    }
    pub fn reverse(mut self, r: bool) -> Self {
        self.reverse = r;
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let ink = self.color.unwrap_or(t.kit.foreground);
        let lit = self.highlight.unwrap_or_else(|| {
            let (target, other) = if t.dark {
                (t.kit.foreground, t.kit.background)
            } else {
                (t.kit.background, t.kit.foreground)
            };
            let l = |c: Color| crate::color::to_hsla(c)[2];
            let target = if (l(target) - l(ink)).abs() < 0.1 {
                other
            } else {
                target
            };
            mix_oklab(ink, target, 0.2)
        });
        let lit = opacity(lit, if t.dark { 0.6 } else { 0.75 });
        let (w, h) = rune::text::measure(&self.text, &[], self.font, None);
        let half = w * self.spread;
        let band = (2.0 * half).max(1.0);
        let pad = half / w.max(1.0) + 0.05;
        // The band's left edge sweeps from -pad·w - half to (1 + pad)·w - half.
        let (x0, x1) = (-pad * w - half, (1.0 + pad) * w - half);
        let (x0, x1) = if self.reverse { (x1, x0) } else { (x0, x1) };
        let slide = |a: f32, b: f32| {
            LoopSpec::slide(self.period, [a, 0.0], [b, 0.0]).easing(rune::Easing::LINEAR)
        };
        let copy = el()
            .absolute(0.0, 0.0)
            .w(band)
            .h(h)
            .looping(slide(-x0 / band, -x1 / band))
            .child(
                text(self.text.clone())
                    .font(self.font)
                    .color(lit)
                    .absolute(0.0, 0.0)
                    .a11y_hidden(),
            );
        let sheen = el()
            .key(format!("{}-sheen", self.key))
            .absolute(0.0, 0.0)
            .w(band)
            .h(h)
            .clip_fade(half, 0.0, half, 0.0)
            .looping(slide(x0 / band, x1 / band))
            .child(copy);
        row()
            .key(self.key)
            .clip_rounded(0.0)
            .child(text(self.text).font(self.font).color(ink))
            .child(sheen)
    }
}
