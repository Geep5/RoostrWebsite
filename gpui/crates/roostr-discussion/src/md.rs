//! Chat message markdown (`renderMarkdown` + the Discussion's `.md` styles and
//! `mentionPills`): `roostr_core::markdown` parses, this lays the blocks out
//! and paints inline styles, links, object links and @agent mention pills.

use std::ops::Range;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::markdown::{Align, Block, Inline, TaskCheck, parse};
use roostr_core::nav::{self, Route};
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::{Store, theme};

/// Colors of the bubble the text sits in.
pub(crate) struct MdTheme {
    /// Body text color (bubble ink in a colored bubble).
    pub ink: Hsla,
    /// A colored bubble's tint (inline code, object links); `None` = the default grey bubble.
    pub tint: Option<Hsla>,
    /// Known agents (name, icon): `@Name` becomes a pill.
    pub mention_names: Vec<(String, String)>,
}

const OBJ_LINK_BG: u32 = 0x2ea04338;
const OBJ_LINK_INK: u32 = 0xb8e6c1;
const MENTION_INK: u32 = 0x1b1b1b;

enum Click {
    Object(String),
    Url(String),
}

/// One rendered line: text, runs and clickable ranges.
struct Line {
    text: String,
    runs: Vec<TextRun>,
    clicks: Vec<(Range<usize>, Click)>,
}

impl Line {
    fn push(&mut self, text: &str, run: &TextRun) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        self.runs.push(TextRun {
            len: text.len(),
            ..run.clone()
        });
    }
}

fn base_run(ink: Hsla) -> TextRun {
    TextRun {
        len: 0,
        font: font(theme::fonts::UI),
        color: ink,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// `(?![\w])` without the unicode flag.
fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Agent names longest first, so "@Bed Test" wins over a "@Bed" agent.
fn mention_at<'a>(text: &str, names: &'a [(String, String)]) -> Option<&'a (String, String)> {
    let rest = text.strip_prefix('@')?;
    names.iter().find(|(name, _)| {
        rest.starts_with(name.as_str()) && !rest[name.len()..].starts_with(word_char)
    })
}

/// An icon usable inside a text run: image icons have no inline glyph, so they fall back.
fn text_icon(icon: &str, fallback: &str) -> String {
    if icon.is_empty() || is_image_icon(icon) {
        fallback.to_string()
    } else {
        icon.to_string()
    }
}

fn build_line(inline: &Inline, md: &MdTheme, names: &[(String, String)], cx: &App) -> Line {
    let mut line = Line {
        text: String::new(),
        runs: Vec::new(),
        clicks: Vec::new(),
    };
    let store = Store::global(cx);
    let store = store.read(cx);
    let space = roostr_core::nav::Navigator::global(cx)
        .read(cx)
        .space()
        .to_string();
    for span in inline {
        let mut run = base_run(md.ink);
        if span.bold {
            run.font.weight = FontWeight::BOLD;
        }
        if span.italic {
            run.font.style = FontStyle::Italic;
        }
        if span.strike {
            run.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
        }
        if span.code {
            run.font = font(theme::fonts::MONO);
            run.background_color = Some(md.tint.unwrap_or(theme::hover()));
        }
        if let Some(link) = &span.link {
            let start = line.text.len();
            match &link.object_id {
                // A linked object: the same pill as a mention, green so it reads as a thing.
                Some(id) => {
                    let summary = store.summary(id);
                    let icon = object_icon(
                        store,
                        summary.map(|s| s.icon.as_str()).unwrap_or(""),
                        summary.map(|s| s.type_key.as_str()).unwrap_or("page"),
                        &space,
                    );
                    let type_icon = object_icon(
                        store,
                        "",
                        summary.map(|s| s.type_key.as_str()).unwrap_or("page"),
                        &space,
                    );
                    run.background_color = Some(md.tint.unwrap_or(rgba(OBJ_LINK_BG).into()));
                    run.color = if md.tint.is_some() {
                        md.ink
                    } else {
                        rgb(OBJ_LINK_INK).into()
                    };
                    line.push(&format!("{}\u{2009}", text_icon(&icon, &type_icon)), &run);
                    line.push(&span.text, &run);
                    line.clicks
                        .push((start..line.text.len(), Click::Object(id.clone())));
                }
                None => {
                    if md.tint.is_some() {
                        run.color = md.ink;
                        run.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(md.ink),
                            wavy: false,
                        });
                    } else {
                        run.color = theme::accent();
                        run.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(theme::accent()),
                            wavy: false,
                        });
                    }
                    line.push(&span.text, &run);
                    line.clicks
                        .push((start..line.text.len(), Click::Url(link.href.clone())));
                }
            }
            continue;
        }
        if span.code || names.is_empty() {
            line.push(&span.text, &run);
            continue;
        }
        // Discord-style mention pills: `@Agent Name` for any known agent.
        let text = span.text.as_str();
        let mut copied = 0;
        let mut i = 0;
        while i < text.len() {
            if text.as_bytes()[i] == b'@'
                && let Some((name, icon)) = mention_at(&text[i..], names)
            {
                line.push(&text[copied..i], &run);
                let pill = TextRun {
                    color: rgb(MENTION_INK).into(),
                    background_color: Some(theme::white()),
                    font: Font {
                        weight: FontWeight::MEDIUM,
                        ..run.font.clone()
                    },
                    ..run.clone()
                };
                line.push(&format!("{}\u{2009}@{name}", text_icon(icon, "🤖")), &pill);
                i += 1 + name.len();
                copied = i;
                continue;
            }
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        line.push(&text[copied..], &run);
    }
    line
}

fn render_line(
    id: ElementId,
    inline: &Inline,
    md: &MdTheme,
    names: &[(String, String)],
    cx: &App,
) -> AnyElement {
    let line = build_line(inline, md, names, cx);
    if line.text.is_empty() {
        // An empty line still takes a line's height (`<br>`).
        return div().child(" ").into_any_element();
    }
    let styled = StyledText::new(line.text).with_runs(line.runs);
    if line.clicks.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, targets): (Vec<Range<usize>>, Vec<Click>) = line.clicks.into_iter().unzip();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| match &targets[ix] {
            Click::Object(id) => nav::navigate(Route::Object(id.clone()), cx),
            Click::Url(url) => cx.open_url(url),
        })
        .into_any_element()
}

/// CSS vertical margins of one block (top, bottom).
fn margins(block: &Block) -> (f32, f32) {
    match block {
        Block::Paragraph(_) => (0., 6.),
        Block::Heading { .. } => (6., 2.),
        Block::Rule => (8., 8.),
        Block::List { .. } | Block::Quote(_) => (4., 4.),
        Block::Code { .. } | Block::Table(_) => (6., 6.),
    }
}

fn lines(
    id: &str,
    ix: usize,
    lines: &[Inline],
    md: &MdTheme,
    names: &[(String, String)],
    cx: &App,
) -> Div {
    div()
        .flex()
        .flex_col()
        .children(lines.iter().enumerate().map(|(i, l)| {
            render_line(
                ElementId::Name(format!("{id}-{ix}-{i}").into()),
                l,
                md,
                names,
                cx,
            )
        }))
}

fn render_block(
    id: &str,
    ix: usize,
    block: &Block,
    md: &MdTheme,
    names: &[(String, String)],
    cx: &App,
) -> AnyElement {
    match block {
        Block::Paragraph(ls) => lines(id, ix, ls, md, names, cx).into_any_element(),
        Block::Heading { level, text } => div()
            .font_weight(FontWeight::BOLD)
            .text_size(px(match level {
                1 => 17.,
                2 => 15.5,
                _ => 14.,
            }))
            .child(render_line(
                ElementId::Name(format!("{id}-{ix}").into()),
                &bold(text),
                md,
                names,
                cx,
            ))
            .into_any_element(),
        Block::Rule => div().h(px(1.)).bg(theme::border()).into_any_element(),
        Block::Quote(ls) => {
            let quote = MdTheme {
                ink: theme::muted(),
                tint: md.tint,
                mention_names: Vec::new(),
            };
            div()
                .border_l_2()
                .border_color(theme::accent())
                .px(px(10.))
                .py(px(2.))
                .text_color(theme::muted())
                .child(lines(id, ix, ls, &quote, names, cx))
                .into_any_element()
        }
        Block::Code { lang, code } => div()
            .id(ElementId::Name(format!("{id}-{ix}-code").into()))
            .flex()
            .flex_col()
            .font_family(theme::fonts::MONO)
            .text_size(px(12.5))
            .line_height(relative(1.5))
            .bg(theme::hover())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::md())
            .px(px(10.))
            .py(px(8.))
            .overflow_x_scroll()
            .when(!lang.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .mb(px(4.))
                        .child(lang.to_uppercase()),
                )
            })
            .child(div().whitespace_nowrap().child(code.clone()))
            .into_any_element(),
        Block::List { ordered, items } => {
            let tasky = items.iter().any(|i| i.check.is_some());
            let rows = items.iter().enumerate().map(|(i, item)| {
                let text = render_line(
                    ElementId::Name(format!("{id}-{ix}-li{i}").into()),
                    &item.text,
                    md,
                    names,
                    cx,
                );
                if tasky {
                    let done = item.check == Some(TaskCheck::Done);
                    let check = div()
                        .flex_none()
                        .size(px(13.))
                        .mt(px(4.))
                        .border(px(1.5))
                        .border_color(md.ink)
                        .opacity(0.7)
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .line_height(px(10.))
                        .when(done, |d| d.child("✓"));
                    div()
                        .flex()
                        .items_start()
                        .gap(px(7.))
                        .when(item.check.is_some(), |d| d.child(check))
                        .child(
                            div()
                                .min_w_0()
                                .when(done, |d| d.opacity(0.6).line_through())
                                .child(text),
                        )
                        .into_any_element()
                } else {
                    let marker = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "•".into()
                    };
                    div()
                        .flex()
                        .child(
                            div()
                                .flex_none()
                                .w(px(20.))
                                .pr(px(6.))
                                .flex()
                                .justify_end()
                                .child(marker),
                        )
                        .child(div().min_w_0().child(text))
                        .into_any_element()
                }
            });
            div()
                .flex()
                .flex_col()
                .when(tasky, |d| d.pl(px(2.)))
                .children(rows)
                .into_any_element()
        }
        Block::Table(table) => {
            let n = table.head.len().max(1);
            let border = theme::border();
            let head_border = border.blend(theme::fg().opacity(0.5));
            let cell = |key: String, inline: &Inline, align: Align, head: bool, last: bool| {
                let theme_for = MdTheme {
                    ink: if head { theme::muted() } else { md.ink },
                    tint: md.tint,
                    mention_names: Vec::new(),
                };
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .when(!last, |d| {
                        d.border_b_1()
                            .border_color(if head { head_border } else { border })
                    })
                    .when(head, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .map(|d| match align {
                        Align::Right => d.text_right().whitespace_nowrap(),
                        Align::Center => d.text_center(),
                        _ => d,
                    })
                    .child(render_line(
                        ElementId::Name(key.into()),
                        &if head { bold(inline) } else { inline.clone() },
                        &theme_for,
                        names,
                        cx,
                    ))
            };
            let rows = table.rows.len();
            let mut grid = div()
                .grid()
                .grid_cols(n as u16)
                .text_size(px(13.))
                .line_height(relative(1.35));
            for (c, h) in table.head.iter().enumerate() {
                grid = grid.child(cell(
                    format!("{id}-{ix}-h{c}"),
                    h,
                    table.aligns.get(c).copied().unwrap_or_default(),
                    true,
                    false,
                ));
            }
            for (r, row) in table.rows.iter().enumerate() {
                for (c, value) in row.iter().enumerate() {
                    grid = grid.child(cell(
                        format!("{id}-{ix}-{r}-{c}"),
                        value,
                        table.aligns.get(c).copied().unwrap_or_default(),
                        false,
                        r + 1 == rows,
                    ));
                }
            }
            div()
                .id(ElementId::Name(format!("{id}-{ix}-table").into()))
                .max_w_full()
                .overflow_x_scroll()
                .child(grid)
                .into_any_element()
        }
    }
}

/// The text as bold (headings, table heads).
fn bold(inline: &Inline) -> Inline {
    inline
        .iter()
        .map(|s| roostr_core::markdown::Span {
            bold: true,
            ..s.clone()
        })
        .collect()
}

/// A message body: parsed markdown laid out with the chat's `.md` styles.
pub(crate) fn render_markdown(
    id: impl Into<ElementId>,
    src: &str,
    md: &MdTheme,
    cx: &App,
) -> AnyElement {
    let id: ElementId = id.into();
    let key = format!("{id:?}");
    let mut names = md.mention_names.clone();
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    let blocks = parse(src);
    let mut out = div().flex().flex_col().min_w_0();
    let mut prev_bottom = 0.;
    let count = blocks.len();
    for (ix, block) in blocks.iter().enumerate() {
        let (top, bottom) = margins(block);
        let gap = if ix == 0 { top } else { top.max(prev_bottom) };
        // `p:last-child { margin-bottom: 0 }`.
        let bottom = if ix + 1 == count && matches!(block, Block::Paragraph(_)) {
            0.
        } else {
            bottom
        };
        prev_bottom = bottom;
        let mut el = div()
            .mt(px(gap))
            .child(render_block(&key, ix, block, md, &names, cx));
        if ix + 1 == count {
            el = el.mb(px(bottom));
        }
        out = out.child(el);
    }
    out.into_any_element()
}
