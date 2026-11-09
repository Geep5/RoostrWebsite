//! Inline rich text the site pages share: the pages' `<b>`, `<i>`, `<code>` and `<a>` runs
//! inside one paragraph, as Rune spans and links.

use std::ops::Range;

use roostr_core::State;
use roostr_core::nav::{self, Route, SitePage};
use rune::{Color, El, Link, Span, TextStyle, Tx, rich};

/// Where a link in rich text goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Href {
    /// A website/app route (`/`, `/guide`, `/app`, ...).
    Route(Route),
    /// Anything else: opened by the system (`https:`, `mailto:`).
    External(String),
}

impl Href {
    pub(crate) fn parse(href: &str) -> Self {
        if href.starts_with('/') {
            Self::Route(Route::parse(href))
        } else {
            Self::External(href.to_string())
        }
    }

    pub(crate) fn open(&self, s: &mut State, tx: &mut Tx) {
        match self {
            Self::Route(route) => nav::navigate(route.clone(), s),
            Self::External(url) => {
                if let Err(error) = tx.open_url(url) {
                    log::warn!("could not open {url}: {error}");
                }
            }
        }
    }
}

/// `href="/"`.
pub(crate) fn home() -> Href {
    Href::Route(Route::Site(SitePage::Landing))
}

/// Colours for the inline styles of one page.
#[derive(Clone, Copy)]
pub(crate) struct RichStyle {
    /// `b { color }` (None: inherit).
    pub bold: Option<Color>,
    /// `a { color }`.
    pub link: Color,
    /// `code { background }`.
    pub code_bg: Color,
}

/// The inherited inline style of a run.
#[derive(Clone, Copy, Default, PartialEq)]
struct Inline {
    bold: bool,
    italic: bool,
    color: Option<Color>,
    underline: Option<Color>,
}

/// One paragraph of inline-styled text.
#[derive(Default, Debug)]
pub(crate) struct Rich {
    pub(crate) text: String,
    pub(crate) spans: Vec<Span>,
    pub(crate) links: Vec<(Range<usize>, String)>,
}

/// A thin space: two each side stand in for `code { padding: 0 6px }`, as box padding inside
/// a line of text takes room but a span background does not.
const THIN: &str = "\u{2009}";

impl Rich {
    /// Parse the page markup: `**bold**`, `_italic_`, `` `code` ``, `[label](href)` (label may
    /// hold `**bold**`). Everything else is literal.
    pub(crate) fn markup(source: &str, style: RichStyle) -> Self {
        let mut rich = Self::default();
        rich.parse(source, style, Inline::default());
        rich
    }

    fn parse(&mut self, source: &str, style: RichStyle, inherited: Inline) {
        let mut rest = source;
        while !rest.is_empty() {
            let next = rest.find(['*', '_', '`', '[']).unwrap_or(rest.len());
            self.push(&rest[..next], inherited);
            rest = &rest[next..];
            if rest.is_empty() {
                break;
            }
            if let Some(body) = rest.strip_prefix("**")
                && let Some(end) = body.find("**")
            {
                let bold = Inline {
                    bold: true,
                    color: style.bold.or(inherited.color),
                    ..inherited
                };
                self.parse(&body[..end], style, bold);
                rest = &body[end + 2..];
            } else if let Some(body) = rest.strip_prefix('_')
                && let Some(end) = body.find('_')
            {
                self.push(
                    &body[..end],
                    Inline {
                        italic: true,
                        ..inherited
                    },
                );
                rest = &body[end + 1..];
            } else if let Some(body) = rest.strip_prefix('`')
                && let Some(end) = body.find('`')
            {
                self.code(&body[..end], style.code_bg);
                rest = &body[end + 1..];
            } else if let Some(body) = rest.strip_prefix('[')
                && let Some(close) = body.find("](")
                && let Some(end) = body[close..].find(')')
            {
                let label = &body[..close];
                let href = &body[close + 2..close + end];
                let start = self.text.len();
                let link = Inline {
                    color: Some(style.link),
                    underline: Some(style.link),
                    ..inherited
                };
                self.parse(label, style, link);
                self.links.push((start..self.text.len(), href.to_string()));
                rest = &body[close + end + 1..];
            } else {
                // A lone marker character is literal text.
                let ch = rest.chars().next().map(char::len_utf8).unwrap_or(1);
                self.push(&rest[..ch], inherited);
                rest = &rest[ch..];
            }
        }
    }

    /// Append `text` in `inline`.
    fn push(&mut self, text: &str, inline: Inline) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        if inline == Inline::default() {
            return;
        }
        let mut span = Span::new(start, self.text.len());
        if inline.bold {
            span = span.weight(700.0);
        }
        if inline.italic {
            span = span.italic();
        }
        if let Some(color) = inline.color {
            span = span.color(color);
        }
        if let Some(color) = inline.underline {
            span = span.underline(color);
        }
        self.spans.push(span);
    }

    /// `<code>`: the mono font on the code background, `padding: 1px 6px`.
    fn code(&mut self, text: &str, background: Color) {
        let start = self.text.len();
        self.text.push_str(&THIN.repeat(2));
        self.spans
            .push(Span::new(start, self.text.len()).bg(background, 1.0));
        let start = self.text.len();
        self.text.push_str(text);
        self.spans
            .push(Span::new(start, self.text.len()).mono().bg(background, 1.0));
        let start = self.text.len();
        self.text.push_str(&THIN.repeat(2));
        self.spans
            .push(Span::new(start, self.text.len()).bg(background, 1.0));
    }

    /// The paragraph as a wrapping text element in `font` and `color`; its links open their
    /// hrefs.
    pub(crate) fn build(self, font: TextStyle, color: Color) -> El {
        let mut e = rich(self.text, self.spans).font(font).color(color);
        if self.links.is_empty() {
            return e;
        }
        for (range, href) in self.links {
            e = e.link(Link::new(range, href));
        }
        e.on_link(|s: &mut State, href: &str, tx: &mut Tx| Href::parse(href).open(s, tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> RichStyle {
        RichStyle {
            bold: Some(Color::WHITE),
            link: Color::BLACK,
            code_bg: Color::TRANSPARENT,
        }
    }

    #[test]
    fn markup_strips_markers_and_styles_runs() {
        let r = Rich::markup("a **b** _c_ `d` [**e** f](/guide) g*h", style());
        assert_eq!(r.text, format!("a b c {THIN}{THIN}d{THIN}{THIN} e f g*h"));
        let b = r.text.find('b').unwrap();
        assert!(
            r.spans
                .iter()
                .any(|s| s.start as usize == b && s.weight == Some(700.0))
        );
        let c = r.text.find('c').unwrap();
        assert!(r.spans.iter().any(|s| s.start as usize == c && s.italic));
        let d = r.text.find('d').unwrap();
        assert!(r.spans.iter().any(|s| s.start as usize == d && s.mono));
        let e = r.text.find('e').unwrap();
        let link = &r.links[0];
        assert_eq!((link.0.start, &r.text[link.0.clone()]), (e, "e f"));
        assert_eq!(
            Href::parse(&link.1),
            Href::Route(Route::Site(SitePage::Guide))
        );
        // The bold run inside a link keeps the link's underline.
        assert!(r.spans.iter().any(|s| s.start as usize == e
            && s.weight == Some(700.0)
            && s.underline == Some(Color::BLACK)));
    }

    #[test]
    fn external_links_stay_urls() {
        assert_eq!(
            Href::parse("mailto:hi@roostr.space"),
            Href::External("mailto:hi@roostr.space".into())
        );
    }
}
