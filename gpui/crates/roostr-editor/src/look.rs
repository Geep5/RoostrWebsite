//! How a text block looks (`BlockNode.svelte` styles): font, size, line
//! height and colour per text style, and the painted runs for its marks
//! (`marks.ts` classes `m-bold` … `m-mention`) as Rune decoration spans.

use std::ops::Range;

use roostr_core::types::{mark, style};
use roostr_ui::Theme;
use rune::{Color, Family, Span as Paint, TextStyle};

use crate::marks::{self, Span};

/// Base typography of one text block.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub style: TextStyle,
    pub color: Color,
}

impl Look {
    /// `.text` + its style class (`.h1` … `.description`, `.done`, inline `color`).
    pub fn for_style(t: &Theme, text_style: u32, checked: bool, color: &str) -> Self {
        let (size, line, weight) = match text_style {
            style::HEADER1 => (28., 1.3, 700.),
            style::HEADER2 => (22., 1.3, 650.),
            style::HEADER3 => (18., 1.3, 600.),
            style::TITLE => (34., 1.55, 750.),
            style::CODE => (13., 1.55, 400.),
            _ => (15., 1.55, 400.),
        };
        let mut fg = t.fg;
        if text_style == style::DESCRIPTION || (text_style == style::CHECKBOX && checked) {
            fg = t.muted;
        }
        if let Some(c) = css_color(color) {
            fg = c;
        }
        let family = if text_style == style::CODE {
            Family::Mono
        } else {
            Family::Sans
        };
        Self {
            style: TextStyle {
                family,
                italic: text_style == style::QUOTE,
                ..TextStyle::new(size, weight).lh(((size * line * 100.0) as f32).round() / 100.)
            },
            color: fg,
        }
    }

    /// A table cell's `.text` (14px / 1.45; header rows 600).
    pub fn table_cell(t: &Theme, header: bool) -> Self {
        Self {
            style: TextStyle::new(14., if header { 600. } else { 400. }).lh(20.3),
            color: t.fg,
        }
    }
}

/// An extra underline over a range (spelling, lint).
#[derive(Clone, Debug)]
pub struct Squiggle {
    pub range: Range<usize>,
    pub color: Color,
}

/// The painted runs for `text`'s marks and extra underlines. Strikethrough
/// and the link press come from the field's own marks (`text::TextState`).
pub fn paints(
    t: &Theme,
    text: &str,
    spans: &[Span],
    look: &Look,
    squiggles: &[Squiggle],
) -> Vec<Paint> {
    let len = text.len();
    if len == 0 || (spans.is_empty() && squiggles.is_empty()) {
        return Vec::new();
    }
    let mut cuts = vec![0, len];
    for m in spans {
        cuts.push(m.from.min(len));
        cuts.push(m.to.min(len));
    }
    for s in squiggles {
        cuts.push(s.range.start.min(len));
        cuts.push(s.range.end.min(len));
    }
    cuts.retain(|c| text.is_char_boundary(*c));
    cuts.sort_unstable();
    cuts.dedup();
    let pieces = marks::to_runs(len, spans);
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let set: &[Span] = pieces
            .iter()
            .find(|(r, _)| r.start <= a && r.end >= b)
            .map(|(_, s)| s.as_slice())
            .unwrap_or(&[]);
        let has = |k: u32| set.iter().find(|m| m.kind == k);
        let squiggle = squiggles
            .iter()
            .rev()
            .find(|s| s.range.start <= a && s.range.end >= b);
        if set.is_empty() && squiggle.is_none() {
            continue;
        }
        let mut run = Paint::new(a, b);
        let mut color = look.color;
        if has(mark::BOLD).is_some() {
            run.weight = Some(look.style.weight.max(700.));
        }
        if has(mark::ITALIC).is_some() {
            run.italic = true;
        }
        if has(mark::INLINE_CODE).is_some() {
            run.mono = true;
            run.bg = Some(t.panel);
        }
        if let Some(c) = has(mark::TEXT_COLOR)
            .and_then(|m| m.param.as_deref())
            .and_then(css_color)
        {
            color = c;
            run.color = Some(c);
        }
        if let Some(c) = has(mark::BACKGROUND_COLOR)
            .and_then(|m| m.param.as_deref())
            .and_then(css_color)
        {
            run.bg = Some(c);
        }
        if has(mark::UNDERLINE).is_some() {
            run.underline = Some(color);
        }
        if has(mark::LINK).is_some() {
            run.color = Some(t.accent);
            run.underline = Some(t.accent);
        }
        if has(mark::MENTION).is_some() {
            run.color = Some(t.accent);
            // CSS: dotted underline, 3px offset - a faint thin line.
            run.underline = Some(t.accent.alpha(0.6));
        }
        if let Some(s) = squiggle {
            run.underline = Some(s.color);
        }
        out.push(run);
    }
    out
}

/// A CSS colour as the web stores it (`#hex`, `rgb()/rgba()`, a few names).
pub fn css_color(raw: &str) -> Option<Color> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(c) = roostr_ui::color::hex(s) {
        return Some(c);
    }
    let lower = s.to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<f32> = inner
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if parts.len() >= 3 {
            let a = parts.get(3).copied().unwrap_or(1.);
            let byte = |v: f32| v.clamp(0., 255.) as u32;
            let alpha = (a.clamp(0., 1.) * 255.).round() as u32;
            return Some(roostr_ui::color::rgba(
                (byte(parts[0]) << 24) | (byte(parts[1]) << 16) | (byte(parts[2]) << 8) | alpha,
            ));
        }
        return None;
    }
    let named: u32 = match lower.as_str() {
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "pink" => 0xffc0cb,
        "gray" | "grey" => 0x808080,
        "white" => 0xffffff,
        "black" => 0x000000,
        _ => return None,
    };
    Some(roostr_ui::color::rgb(named))
}
