//! Chat markdown (`src/lib/markdown.ts`), parsed into a tree instead of HTML:
//! fenced code blocks, inline code, bold, italic, strikethrough, links (+ bare
//! URLs), lists (with task checkboxes), blockquotes, headings (1-3), rules,
//! GFM tables and paragraphs. Same rules and edge cases as the web renderer:
//! inline transforms run in the same order over the same text, so nesting
//! and precedence match.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::{Captures, Regex};

/// A run of text with one combination of styles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Inline code (`code.ic`).
    pub code: bool,
    pub link: Option<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The target: a safe `https:`/`http:`/`mailto:` URL, or `/app/object/<id>`.
    pub href: String,
    /// `Some(id)` for an in-app object link (opens here, not in a browser).
    pub object_id: Option<String>,
}

/// One line of inline content.
pub type Inline = Vec<Span>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskCheck {
    Todo,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// `- [ ] x` / `- [x] x` items (bullet lists only).
    pub check: Option<TaskCheck>,
    pub text: Inline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    /// Explicit `---:`, or a column whose every non-empty cell is numeric
    /// (render no-wrap, like the web's `td.num`).
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub head: Vec<Inline>,
    /// One per head column.
    pub aligns: Vec<Align>,
    /// Every row has exactly `head.len()` cells.
    pub rows: Vec<Vec<Inline>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Lines joined by hard breaks.
    Paragraph(Vec<Inline>),
    /// Level 1..=3 (`.md-h1`..`.md-h3`).
    Heading {
        level: u8,
        text: Inline,
    },
    Rule,
    /// A list with any task item is a task list (`.md-tasks`).
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    /// Lines joined by hard breaks.
    Quote(Vec<Inline>),
    Code {
        lang: String,
        code: String,
    },
    Table(Table),
}

// Private-use markers standing in for the web's HTML tags during the inline pass.
const B_OPEN: char = '\u{E000}';
const B_CLOSE: char = '\u{E001}';
const I_OPEN: char = '\u{E002}';
const I_CLOSE: char = '\u{E003}';
const S_OPEN: char = '\u{E004}';
const S_CLOSE: char = '\u{E005}';
const L_OPEN: char = '\u{E006}'; // followed by the link index, then L_MID
const L_MID: char = '\u{E007}';
const L_CLOSE: char = '\u{E008}';
const C_OPEN: char = '\u{E009}'; // followed by the code index, then C_CLOSE
const C_CLOSE: char = '\u{E00A}';

/// JavaScript's `.` (no line terminators).
const DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

fn re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("{DOT}", DOT)).expect("valid regex")
}

static CODE: LazyLock<Regex> = LazyLock::new(|| re(r"`([^`\n]+)`"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*([^*]+)\*\*"));
static ITALIC_STAR: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<![A-Za-z0-9_])\*([^*\n]+)\*(?![A-Za-z0-9_])").expect("valid")
});
static ITALIC_UNDER: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<![A-Za-z0-9_])_([^_\n]+)_(?![A-Za-z0-9_])").expect("valid")
});
static STRIKE: LazyLock<Regex> = LazyLock::new(|| re(r"~~([^~]+)~~"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]\n]+)\]\(([^)\s]+)\)"));
static OBJECT_HREF: LazyLock<Regex> = LazyLock::new(|| re(r"^/app/object/([A-Za-z0-9_-]+)$"));
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(^|[\s(])(https?://[^\s<>()\x00\u{E000}-\u{E00A}]+[^\s<>().,;:!?'"\x00\u{E000}-\u{E00A}])"#,
    )
});
static CODE_SLOT: LazyLock<Regex> = LazyLock::new(|| re(r"\x00C(\d+)\x00"));
static SAFE_SCHEME: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(https?:|mailto:)"));
static BARE_DOMAIN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^[\w.-]+\.[a-z]{2,}(/\S*)?$"));

static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)```([A-Za-z0-9_]*)\n?(.*?)(?:```|\z)"));
static BLOCK_SLOT: LazyLock<Regex> = LazyLock::new(|| re(r"^\x00B(\d+)\x00$"));
static ROW: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\|{DOT}*\|\s*$"));
static SEP_CELL: LazyLock<Regex> = LazyLock::new(|| re(r"^:?-+:?$"));
static NUMERIC: LazyLock<Regex> = LazyLock::new(|| re(r"^[-+]?[$€£¥]?\s?[0-9,.]+%?[kKmM]?$"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^(#{1,3})\s+({DOT}*)$"));
static HR: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-*_]{3,}\s*$"));
static UL: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-*]\s+({DOT}*)$"));
static OL: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[0-9]+[.)]\s+({DOT}*)$"));
static TASK: LazyLock<Regex> = LazyLock::new(|| re(r"^\[( |x|X)\]\s+({DOT}*)$"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"^>\s?({DOT}*)$"));

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape_html(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// `safeHref`: http(s)/mailto as is, a bare domain as https, anything else "".
pub fn safe_href(url: &str) -> String {
    let u = url.trim();
    if SAFE_SCHEME.is_match(u) {
        return u.to_string();
    }
    if BARE_DOMAIN.is_match(u) {
        return format!("https://{u}");
    }
    String::new()
}

fn fancy_wrap(re: &FancyRegex, s: &str, open: char, close: char) -> String {
    re.replace_all(s, |c: &fancy_regex::Captures| {
        format!("{open}{}{close}", &c[1])
    })
    .into_owned()
}

/// Parse one line of inline markdown.
pub fn parse_inline(s: &str) -> Inline {
    let mut out = escape_html(s);
    // Inline code first: its contents are protected from the other transforms.
    let mut codes: Vec<String> = Vec::new();
    out = CODE
        .replace_all(&out, |c: &Captures| {
            codes.push(c[1].to_string());
            format!("\x00C{}\x00", codes.len() - 1)
        })
        .into_owned();
    out = BOLD
        .replace_all(&out, format!("{B_OPEN}$1{B_CLOSE}").as_str())
        .into_owned();
    out = fancy_wrap(&ITALIC_STAR, &out, I_OPEN, I_CLOSE);
    out = fancy_wrap(&ITALIC_UNDER, &out, I_OPEN, I_CLOSE);
    out = STRIKE
        .replace_all(&out, format!("{S_OPEN}$1{S_CLOSE}").as_str())
        .into_owned();
    let mut links: Vec<Link> = Vec::new();
    out = LINK
        .replace_all(&out, |c: &Captures| {
            let (text, url) = (&c[1], &c[2]);
            let link = if let Some(m) = OBJECT_HREF.captures(url) {
                Link {
                    href: url.to_string(),
                    object_id: Some(m[1].to_string()),
                }
            } else {
                let href = safe_href(&url.replace("&amp;", "&"));
                if href.is_empty() {
                    return text.to_string();
                }
                Link {
                    href,
                    object_id: None,
                }
            };
            links.push(link);
            format!("{L_OPEN}{}{L_MID}{text}{L_CLOSE}", links.len() - 1)
        })
        .into_owned();
    out = BARE_URL
        .replace_all(&out, |c: &Captures| {
            links.push(Link {
                href: unescape_html(&c[2]),
                object_id: None,
            });
            format!(
                "{}{L_OPEN}{}{L_MID}{}{L_CLOSE}",
                &c[1],
                links.len() - 1,
                &c[2]
            )
        })
        .into_owned();
    out = CODE_SLOT
        .replace_all(&out, format!("{C_OPEN}$1{C_CLOSE}").as_str())
        .into_owned();
    spans(&out, &codes, &links)
}

/// Turn the marked-up text into styled spans.
fn spans(marked: &str, codes: &[String], links: &[Link]) -> Inline {
    let mut out: Inline = Vec::new();
    let (mut bold, mut italic, mut strike) = (0u32, 0u32, 0u32);
    let mut link_stack: Vec<Option<Link>> = Vec::new();
    let mut text = String::new();
    let current = |bold: u32, italic: u32, strike: u32, link_stack: &[Option<Link>]| Span {
        text: String::new(),
        bold: bold > 0,
        italic: italic > 0,
        strike: strike > 0,
        code: false,
        link: link_stack.iter().rev().find_map(Clone::clone),
    };
    let flush = |text: &mut String, out: &mut Inline, span: Span| {
        if !text.is_empty() {
            out.push(Span {
                text: unescape_html(text),
                ..span
            });
            text.clear();
        }
    };
    let mut chars = marked.chars().peekable();
    while let Some(c) = chars.next() {
        let is_marker = matches!(c, B_OPEN..=C_CLOSE);
        if !is_marker {
            text.push(c);
            continue;
        }
        flush(
            &mut text,
            &mut out,
            current(bold, italic, strike, &link_stack),
        );
        let mut index = || {
            let mut n = String::new();
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                n.push(d);
            }
            chars.next(); // L_MID / C_CLOSE
            n.parse::<usize>().ok()
        };
        match c {
            B_OPEN => bold += 1,
            B_CLOSE => bold = bold.saturating_sub(1),
            I_OPEN => italic += 1,
            I_CLOSE => italic = italic.saturating_sub(1),
            S_OPEN => strike += 1,
            S_CLOSE => strike = strike.saturating_sub(1),
            L_OPEN => {
                let link = index().and_then(|i| links.get(i).cloned());
                link_stack.push(link);
            }
            L_CLOSE => {
                link_stack.pop();
            }
            C_OPEN => {
                if let Some(code) = index().and_then(|i| codes.get(i)) {
                    let span = current(bold, italic, strike, &link_stack);
                    out.push(Span {
                        text: unescape_html(code),
                        code: true,
                        ..span
                    });
                }
            }
            _ => {}
        }
    }
    flush(
        &mut text,
        &mut out,
        current(bold, italic, strike, &link_stack),
    );
    out
}

fn is_row(line: &str) -> bool {
    ROW.is_match(line)
}

/// A row's cells, outer pipes dropped; `\|` stays a literal pipe.
fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut prev = '\0';
    for c in t.chars() {
        if c == '|' && prev != '\\' {
            out.push(std::mem::take(&mut cell));
        } else {
            cell.push(c);
        }
        prev = c;
    }
    out.push(cell);
    out.into_iter()
        .map(|c| c.trim().replace("\\|", "|"))
        .collect()
}

fn is_separator(line: &str) -> bool {
    is_row(line) && cells(line).iter().all(|c| SEP_CELL.is_match(c))
}

fn numeric(c: &str) -> bool {
    NUMERIC.is_match(c.trim())
}

fn table(head: Vec<String>, aligns: Vec<Align>, rows: Vec<Vec<String>>) -> Table {
    let aligns = (0..head.len())
        .map(|i| match aligns.get(i).copied().unwrap_or_default() {
            Align::None
                if !rows.is_empty()
                    && rows
                        .iter()
                        .all(|r| r.get(i).is_none_or(|c| c.is_empty() || numeric(c))) =>
            {
                Align::Right
            }
            a => a,
        })
        .collect();
    let width = head.len();
    Table {
        aligns,
        rows: rows
            .iter()
            .map(|r| {
                (0..width)
                    .map(|i| parse_inline(r.get(i).map_or("", String::as_str)))
                    .collect()
            })
            .collect(),
        head: head.iter().map(|h| parse_inline(h)).collect(),
    }
}

/// Parse a chat message (`renderMarkdown`).
pub fn parse(src: &str) -> Vec<Block> {
    // 1) fenced code blocks
    let mut codes: Vec<Block> = Vec::new();
    let text = FENCE.replace_all(src, |c: &Captures| {
        let code = &c[2];
        codes.push(Block::Code {
            lang: c[1].to_string(),
            code: code.strip_suffix('\n').unwrap_or(code).to_string(),
        });
        format!("\n\x00B{}\x00\n", codes.len() - 1)
    });

    // 2) block structure
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut list: Option<(bool, Vec<ListItem>)> = None;
    let mut quote: Vec<String> = Vec::new();
    fn flush_para(para: &mut Vec<&str>, out: &mut Vec<Block>) {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.drain(..).map(parse_inline).collect()));
        }
    }
    fn flush_list(list: &mut Option<(bool, Vec<ListItem>)>, out: &mut Vec<Block>) {
        if let Some((ordered, items)) = list.take() {
            out.push(Block::List { ordered, items });
        }
    }
    fn flush_quote(quote: &mut Vec<String>, out: &mut Vec<Block>) {
        if !quote.is_empty() {
            out.push(Block::Quote(
                quote.drain(..).map(|q| parse_inline(&q)).collect(),
            ));
        }
    }

    let mut li = 0;
    while li < lines.len() {
        let line = lines[li];
        li += 1;
        if let Some(m) = BLOCK_SLOT.captures(line) {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            flush_quote(&mut quote, &mut out);
            if let Some(block) = m[1].parse::<usize>().ok().and_then(|i| codes.get(i)) {
                out.push(block.clone());
            }
            continue;
        }
        // GFM table: a | row, a |---| separator, then rows until one isn't a | row.
        if is_row(line) && li < lines.len() && is_separator(lines[li]) {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            flush_quote(&mut quote, &mut out);
            let head = cells(line);
            let aligns = cells(lines[li])
                .iter()
                .map(|c| {
                    let (l, r) = (c.starts_with(':'), c.ends_with(':') && c.len() > 1);
                    match (l, r) {
                        (true, true) => Align::Center,
                        (false, true) => Align::Right,
                        (true, false) => Align::Left,
                        (false, false) => Align::None,
                    }
                })
                .collect();
            li += 1;
            let mut rows = Vec::new();
            while li < lines.len() && is_row(lines[li]) {
                rows.push(cells(lines[li]));
                li += 1;
            }
            out.push(Block::Table(table(head, aligns, rows)));
            continue;
        }
        if let Some(h) = HEADING.captures(line) {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            flush_quote(&mut quote, &mut out);
            out.push(Block::Heading {
                level: h[1].len() as u8,
                text: parse_inline(&h[2]),
            });
            continue;
        }
        if HR.is_match(line) {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            flush_quote(&mut quote, &mut out);
            out.push(Block::Rule);
            continue;
        }
        let ul = UL.captures(line);
        let ol = if ul.is_none() {
            OL.captures(line)
        } else {
            None
        };
        if let Some(m) = ul.as_ref().or(ol.as_ref()) {
            flush_para(&mut para, &mut out);
            flush_quote(&mut quote, &mut out);
            let ordered = ul.is_none();
            if list.as_ref().is_none_or(|(o, _)| *o != ordered) {
                flush_list(&mut list, &mut out);
                list = Some((ordered, Vec::new()));
            }
            let mut item = m[1].to_string();
            let mut check = None;
            if !ordered && let Some(task) = TASK.captures(&m[1]) {
                check = Some(if &task[1] == " " {
                    TaskCheck::Todo
                } else {
                    TaskCheck::Done
                });
                item = task[2].to_string();
            }
            if let Some((_, items)) = list.as_mut() {
                items.push(ListItem {
                    check,
                    text: parse_inline(&item),
                });
            }
            continue;
        }
        if let Some(q) = QUOTE.captures(line) {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            quote.push(q[1].to_string());
            continue;
        }
        if line.trim().is_empty() {
            flush_para(&mut para, &mut out);
            flush_list(&mut list, &mut out);
            flush_quote(&mut quote, &mut out);
            continue;
        }
        flush_list(&mut list, &mut out);
        flush_quote(&mut quote, &mut out);
        para.push(line);
    }
    flush_para(&mut para, &mut out);
    flush_list(&mut list, &mut out);
    flush_quote(&mut quote, &mut out);
    out
}

/// The plain text of a line (styles and links dropped).
pub fn plain_text(inline: &Inline) -> String {
    inline.iter().map(|s| s.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }

    #[test]
    fn inline_styles_nest() {
        let line = parse_inline("a **b _c_ d** `x*y*` ~~s~~ <i>");
        assert_eq!(line[0], span("a "));
        assert_eq!(
            line[1],
            Span {
                bold: true,
                ..span("b ")
            }
        );
        assert_eq!(
            line[2],
            Span {
                bold: true,
                italic: true,
                ..span("c")
            }
        );
        assert_eq!(
            line[3],
            Span {
                bold: true,
                ..span(" d")
            }
        );
        assert_eq!(
            line[5],
            Span {
                code: true,
                ..span("x*y*")
            }
        );
        assert_eq!(
            line[7],
            Span {
                strike: true,
                ..span("s")
            }
        );
        assert_eq!(line[8], span(" <i>"));
        // Word-internal underscores stay literal.
        assert_eq!(
            parse_inline("snake_case_name"),
            vec![span("snake_case_name")]
        );
    }

    #[test]
    fn links() {
        let line = parse_inline(
            "see [docs](example.com/a?b=1&c=2) and https://x.io/p. [o](/app/object/abc-1) [bad](javascript:x)",
        );
        assert_eq!(
            line[1].link.as_ref().unwrap().href,
            "https://example.com/a?b=1&c=2"
        );
        assert_eq!(line[1].text, "docs");
        assert_eq!(line[3].link.as_ref().unwrap().href, "https://x.io/p");
        assert_eq!(line[4].text, ". ");
        assert_eq!(
            line[5].link.as_ref().unwrap().object_id.as_deref(),
            Some("abc-1")
        );
        assert_eq!(line[6], span(" bad"));
    }

    #[test]
    fn blocks() {
        let src = "# Title\npara one\nline two\n\n- [ ] todo\n- [x] done\n1. first\n> quoted\n---\n```rs\nfn x() {}\n```\n| Item | Cost |\n|---|:-:|\n| a | $1,200 |\n| b \\| c | 3 |";
        let blocks = parse(src);
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 1,
                text: vec![span("Title")]
            }
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph(vec![vec![span("para one")], vec![span("line two")]])
        );
        let Block::List {
            ordered: false,
            items,
        } = &blocks[2]
        else {
            panic!("{:?}", blocks[2])
        };
        assert_eq!(items[0].check, Some(TaskCheck::Todo));
        assert_eq!(items[1].check, Some(TaskCheck::Done));
        assert!(matches!(&blocks[3], Block::List { ordered: true, .. }));
        assert_eq!(blocks[4], Block::Quote(vec![vec![span("quoted")]]));
        assert_eq!(blocks[5], Block::Rule);
        assert_eq!(
            blocks[6],
            Block::Code {
                lang: "rs".into(),
                code: "fn x() {}".into()
            }
        );
        let Block::Table(t) = &blocks[7] else {
            panic!("{:?}", blocks[7])
        };
        assert_eq!(t.aligns, vec![Align::None, Align::Center]);
        assert_eq!(plain_text(&t.rows[1][0]), "b | c");
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn numeric_columns_align_right() {
        let Block::Table(t) = &parse("| a | n |\n|---|---|\n| x | 12% |\n| y | |")[0] else {
            panic!()
        };
        assert_eq!(t.aligns, vec![Align::None, Align::Right]);
    }
}
