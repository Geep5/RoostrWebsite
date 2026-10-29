//! Text helpers the site pages share: inline rich text (the pages' `<b>`,
//! `<i>`, `<code>`, `<a>` runs inside one paragraph) and letter-spaced
//! single lines (CSS `letter-spacing`, which GPUI text has no notion of).

use std::ops::Range;

use gpui_kit::*;
use roostr_core::nav::{self, Route, SitePage};

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

    pub(crate) fn open(&self, cx: &mut App) {
        match self {
            Self::Route(route) => nav::navigate(route.clone(), cx),
            Self::External(url) => cx.open_url(url),
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
    pub bold: Option<Hsla>,
    /// `a { color }`.
    pub link: Hsla,
    /// `code { background }`.
    pub code_bg: Hsla,
}

/// One paragraph of inline-styled text.
#[derive(Default)]
pub(crate) struct Rich {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    fonts: Vec<(Range<usize>, SharedString)>,
    links: Vec<(Range<usize>, Href)>,
}

/// A thin space: stands in for `code { padding: 0 6px }` / pill padding, as
/// GPUI text runs cannot carry box padding.
const THIN: &str = "\u{2009}";

impl Rich {
    /// Parse the page markup: `**bold**`, `_italic_`, `` `code` ``,
    /// `[label](href)` (label may hold `**bold**`). Everything else is literal.
    pub(crate) fn markup(source: &str, style: RichStyle) -> Self {
        let mut rich = Self::default();
        rich.parse(source, style, None);
        rich
    }

    fn parse(&mut self, source: &str, style: RichStyle, inherited: Option<HighlightStyle>) {
        let mut rest = source;
        while !rest.is_empty() {
            let next = rest.find(['*', '_', '`', '[']).unwrap_or(rest.len());
            self.push(&rest[..next], inherited.unwrap_or_default());
            rest = &rest[next..];
            if rest.is_empty() {
                break;
            }
            if let Some(body) = rest.strip_prefix("**")
                && let Some(end) = body.find("**")
            {
                let mut bold = inherited.unwrap_or_default();
                bold.font_weight = Some(FontWeight::BOLD);
                if let Some(color) = style.bold {
                    bold.color = Some(color);
                }
                self.parse(&body[..end], style, Some(bold));
                rest = &body[end + 2..];
            } else if let Some(body) = rest.strip_prefix('_')
                && let Some(end) = body.find('_')
            {
                let mut italic = inherited.unwrap_or_default();
                italic.font_style = Some(FontStyle::Italic);
                self.push(&body[..end], italic);
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
                let link = HighlightStyle {
                    color: Some(style.link),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(style.link),
                        wavy: false,
                    }),
                    ..inherited.unwrap_or_default()
                };
                self.parse(label, style, Some(link));
                self.links.push((start..self.text.len(), Href::parse(href)));
                rest = &body[close + end + 1..];
            } else {
                // A lone marker character is literal text.
                let ch = rest.chars().next().map(char::len_utf8).unwrap_or(1);
                self.push(&rest[..ch], inherited.unwrap_or_default());
                rest = &rest[ch..];
            }
        }
    }

    /// Append `text` with `highlight`.
    pub(crate) fn push(&mut self, text: &str, highlight: HighlightStyle) -> &mut Self {
        if text.is_empty() {
            return self;
        }
        let start = self.text.len();
        push_breakable(&mut self.text, text);
        if highlight != HighlightStyle::default() {
            self.highlights.push((start..self.text.len(), highlight));
        }
        self
    }


    /// `<code>`: system mono on the code background, `padding: 1px 6px`.
    /// The background goes on three runs (pad, code, pad): a font-family
    /// override applies only to runs that lie wholly inside its range.
    fn code(&mut self, text: &str, background: Hsla) {
        let padding = THIN.repeat(2);
        let highlight = HighlightStyle {
            background_color: Some(background),
            ..Default::default()
        };
        for (part, is_code) in [
            (padding.as_str(), false),
            (text, true),
            (padding.as_str(), false),
        ] {
            let start = self.text.len();
            self.text.push_str(part);
            self.highlights.push((start..self.text.len(), highlight));
            if is_code {
                self.fonts.push((
                    start..self.text.len(),
                    roostr_core::theme::fonts::MONO.into(),
                ));
            }
        }
    }

    /// The paragraph as an element (clickable when it has links).
    pub(crate) fn into_element(self, id: impl Into<ElementId>) -> AnyElement {
        let mut highlights = self.highlights;
        // StyledText wants sorted, non-overlapping ranges: nested markup
        // pushes outer runs only around their plain parts, so a sort suffices.
        highlights.sort_by_key(|(range, _)| range.start);
        let styled = StyledText::new(self.text)
            .with_highlights(highlights)
            .with_font_family_overrides(self.fonts);
        if self.links.is_empty() {
            return styled.into_any_element();
        }
        let (ranges, hrefs): (Vec<_>, Vec<_>) = self.links.into_iter().unzip();
        InteractiveText::new(id, styled)
            .on_click(ranges, move |ix, _, cx| hrefs[ix].open(cx))
            .into_any_element()
    }
}

/// Append `text`, letting lines break after a hyphen that joins two words
/// ("Task-like", "end-to-end") like the browser does (UAX #14: no break
/// before a digit). GPUI keeps `-` inside words, so a zero-width space marks
/// the opportunity.
fn push_breakable(out: &mut String, text: &str) {
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        out.push(ch);
        if ch == '-'
            && previous.is_some_and(|p| p.is_alphanumeric() || p == '@')
            && chars.peek().is_some_and(|n| n.is_alphabetic())
        {
            out.push('\u{200B}');
        }
        previous = Some(ch);
    }
}
