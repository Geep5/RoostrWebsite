//! How a text block looks (`BlockNode.svelte` styles): font, size, line
//! height and colour per text style, and the text runs for its marks
//! (`marks.ts` classes `m-bold` … `m-mention`).

use std::ops::Range;

use gpui_kit::*;
use roostr_core::theme;
use roostr_core::types::{mark, style};

use crate::marks::{self, Span};

/// Base typography of one text block.
#[derive(Clone)]
pub struct Look {
    pub family: SharedString,
    pub size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
    pub italic: bool,
    pub color: Hsla,
}

impl Look {
    /// `.text` + its style class (`.h1` … `.description`, `.done`, inline `color`).
    pub fn for_style(text_style: u32, checked: bool, color: &str) -> Self {
        let (size, line, weight) = match text_style {
            style::HEADER1 => (28., 1.3, 700.),
            style::HEADER2 => (22., 1.3, 650.),
            style::HEADER3 => (18., 1.3, 600.),
            style::TITLE => (34., 1.55, 750.),
            style::CODE => (13., 1.55, 400.),
            _ => (15., 1.55, 400.),
        };
        let mut fg = theme::fg();
        if text_style == style::DESCRIPTION || (text_style == style::CHECKBOX && checked) {
            fg = theme::muted();
        }
        if let Some(c) = css_color(color) {
            fg = c;
        }
        Self {
            family: if text_style == style::CODE {
                theme::fonts::MONO.into()
            } else {
                theme::fonts::UI.into()
            },
            size: px(size),
            line_height: px((size * line * 100.).round() / 100.),
            weight: FontWeight(weight),
            italic: text_style == style::QUOTE,
            color: fg,
        }
    }

    /// A table cell's `.text` (14px / 1.45; header rows 600).
    pub fn table_cell(header: bool) -> Self {
        Self {
            family: theme::fonts::UI.into(),
            size: px(14.),
            line_height: px(20.3),
            weight: FontWeight(if header { 600. } else { 400. }),
            italic: false,
            color: theme::fg(),
        }
    }

    fn font(&self, bold: bool, italic: bool, mono: bool) -> Font {
        Font {
            family: if mono {
                theme::fonts::MONO.into()
            } else {
                self.family.clone()
            },
            features: FontFeatures::default(),
            fallbacks: None,
            weight: if bold {
                FontWeight(self.weight.0.max(700.))
            } else {
                self.weight
            },
            style: if italic || self.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }

    pub fn plain_run(&self, len: usize, color: Hsla) -> TextRun {
        TextRun {
            len,
            font: self.font(false, false, false),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }
}

/// An extra underline over a range (spelling, lint, input-method composition).
#[derive(Clone)]
pub struct Squiggle {
    pub range: Range<usize>,
    pub color: Hsla,
    pub thickness: Pixels,
    pub wavy: bool,
}

/// Text runs for `text` with its marks and extra underlines.
pub fn runs(text: &str, spans: &[Span], look: &Look, squiggles: &[Squiggle]) -> Vec<TextRun> {
    let len = text.len();
    if len == 0 {
        return vec![look.plain_run(0, look.color)];
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
        let mono = has(mark::INLINE_CODE).is_some();
        let mut run = TextRun {
            len: b - a,
            font: look.font(has(mark::BOLD).is_some(), has(mark::ITALIC).is_some(), mono),
            color: look.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if mono {
            run.background_color = Some(theme::panel());
        }
        if let Some(c) = has(mark::TEXT_COLOR)
            .and_then(|m| m.param.as_deref())
            .and_then(css_color)
        {
            run.color = c;
        }
        if let Some(c) = has(mark::BACKGROUND_COLOR)
            .and_then(|m| m.param.as_deref())
            .and_then(css_color)
        {
            run.background_color = Some(c);
        }
        if has(mark::UNDERLINE).is_some() {
            run.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(run.color),
                wavy: false,
            });
        }
        if has(mark::STRIKETHROUGH).is_some() {
            run.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(run.color),
            });
        }
        if has(mark::LINK).is_some() {
            run.color = theme::accent();
            run.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme::accent()),
                wavy: false,
            });
        }
        if has(mark::MENTION).is_some() {
            run.color = theme::accent();
            // CSS: dotted underline, 3px offset - a faint thin line is the nearest GPUI underline.
            run.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme::accent().opacity(0.6)),
                wavy: false,
            });
        }
        if let Some(s) = squiggles
            .iter()
            .rev()
            .find(|s| s.range.start <= a && s.range.end >= b)
        {
            run.underline = Some(UnderlineStyle {
                thickness: s.thickness,
                color: Some(s.color),
                wavy: s.wavy,
            });
        }
        out.push(run);
    }
    out
}

/// A CSS colour as the web stores it (`#hex`, `rgb()/rgba()`, a few names).
pub fn css_color(raw: &str) -> Option<Hsla> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(c) = roostr_core::color::hex(s) {
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
            return Some(
                Rgba {
                    r: parts[0] / 255.,
                    g: parts[1] / 255.,
                    b: parts[2] / 255.,
                    a,
                }
                .into(),
            );
        }
        return None;
    }
    let named = match lower.as_str() {
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
    Some(rgb(named).into())
}
