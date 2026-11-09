//! Chat message markdown (`renderMarkdown` + the Discussion's `.md` styles and
//! `mentionPills`): `roostr_core::markdown` parses, this lays the blocks out
//! and paints inline styles, links, object links and @agent mention pills.
//! Every line is selectable text in the conversation's one selection group, as
//! the web view's DOM text was: a drag selects across lines and messages, ⌘C
//! copies.

use roostr_core::Store;
use roostr_core::markdown::{Align, Block, Inline, Link, Span, TaskCheck, parse};
use roostr_core::objects::object_icon;
use roostr_ui::tags::{TagPaint, tint_paint};
use roostr_ui::{Theme, is_image_icon, radius, ty};
use rune::{Color, El, TextStyle, col, rich, row, text};

use crate::discussion::{OBJECT_GREEN, agent_tag_paint};

/// Colours of the bubble the text sits in.
pub(crate) struct MdTheme {
    /// Body text colour (bubble ink in a coloured bubble).
    pub ink: Color,
    /// A coloured bubble's tint (inline code, object links); `None` = the default grey bubble.
    pub tint: Option<Color>,
    /// Known agents (name, icon): `@Name` becomes a pill.
    pub mention_names: Vec<(String, String)>,
}

/// What a message body is laid out against: the theme, the store (object links' icons, agent
/// tag colours) and the active space.
pub(crate) struct MdEnv<'a> {
    pub t: &'a Theme,
    pub store: &'a Store,
    pub space: &'a str,
}

/// A link's id as `on_link` hears it: an object to open here, or a web address.
pub(crate) const OBJECT_LINK: &str = "object:";
pub(crate) const URL_LINK: &str = "url:";

/// One laid-out line: text, styled ranges, the ranges drawn as tags (fill, 1px ring) and the
/// links.
#[derive(Default)]
struct Line {
    text: String,
    spans: Vec<rune::Span>,
    rings: Vec<(usize, usize, Color, f32)>,
    strikes: Vec<(usize, usize, Option<Color>)>,
    links: Vec<(std::ops::Range<usize>, String)>,
}

/// How a run of text is painted.
#[derive(Clone, Copy)]
struct Run {
    color: Color,
    weight: Option<f32>,
    italic: bool,
    mono: bool,
    bg: Option<(Color, f32)>,
    underline: Option<Color>,
    strike: bool,
}

impl Run {
    fn plain(color: Color) -> Run {
        Run {
            color,
            weight: None,
            italic: false,
            mono: false,
            bg: None,
            underline: None,
            strike: false,
        }
    }
}

impl Line {
    /// Appends `text` painted as `run`; returns its byte range.
    fn push(&mut self, text: &str, run: Run) -> std::ops::Range<usize> {
        let start = self.text.len();
        if text.is_empty() {
            return start..start;
        }
        self.text.push_str(text);
        let end = self.text.len();
        let mut span = rune::Span::new(start, end).color(run.color);
        if let Some(w) = run.weight {
            span = span.weight(w);
        }
        if run.italic {
            span = span.italic();
        }
        if run.mono {
            span = span.mono();
        }
        if let Some((bg, pad)) = run.bg {
            span = span.bg(bg, pad);
        }
        if let Some(u) = run.underline {
            span = span.underline(u);
        }
        if run.strike {
            self.strikes.push((start, end, None));
        }
        self.spans.push(span);
        start..end
    }

    /// The kit tag look behind `range`: fill, 1px border, 2px past the glyphs.
    fn tag(&mut self, range: std::ops::Range<usize>, paint: TagPaint) {
        for span in self.spans.iter_mut() {
            if span.start as usize >= range.start && span.end as usize <= range.end {
                *span = span.clone().bg(paint.bg, 2.0);
            }
        }
        self.rings.push((range.start, range.end, paint.border, 1.0));
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

fn build_line(inline: &Inline, md: &MdTheme, names: &[(String, String)], env: &MdEnv) -> Line {
    let t = env.t;
    let mut line = Line::default();
    for span in inline {
        let mut run = Run::plain(md.ink);
        if span.bold {
            run.weight = Some(ty::BOLD);
        }
        run.italic = span.italic;
        run.strike = span.strike;
        if span.code {
            run.mono = true;
            run.bg = Some((md.tint.unwrap_or(t.hover), 0.0));
        }
        if let Some(link) = &span.link {
            match &link.object_id {
                // A linked object: the same pill as a mention, green so it reads as a thing.
                Some(id) => {
                    let summary = env.store.summary(id);
                    let type_key = summary.map(|s| s.type_key.as_str()).unwrap_or("page");
                    let icon = object_icon(
                        env.store,
                        summary.map(|s| s.icon.as_str()).unwrap_or(""),
                        type_key,
                        env.space,
                    );
                    let type_icon = object_icon(env.store, "", type_key, env.space);
                    // The composer's !object tag: the kit tag look in green (a coloured
                    // bubble keeps its own tint).
                    let paint = match md.tint {
                        Some(tint) => {
                            run.bg = Some((tint, 2.0));
                            run.color = md.ink;
                            None
                        }
                        None => {
                            let paint = tint_paint(t, roostr_ui::color::rgb(OBJECT_GREEN));
                            run.color = paint.fg;
                            Some(paint)
                        }
                    };
                    // A narrow no-break space: the icon stays with the name.
                    let start = line.text.len();
                    line.push(&format!("{}\u{202f}", text_icon(&icon, &type_icon)), run);
                    let end = line.push(&span.text, run).end.max(line.text.len());
                    if let Some(paint) = paint {
                        line.tag(start..end, paint);
                    }
                    line.links.push((start..end, format!("{OBJECT_LINK}{id}")));
                }
                None => {
                    let color = if md.tint.is_some() { md.ink } else { t.accent };
                    run.color = color;
                    run.underline = Some(color);
                    let range = line.push(&span.text, run);
                    line.links.push((range, format!("{URL_LINK}{}", link.href)));
                }
            }
            continue;
        }
        if span.code || names.is_empty() {
            line.push(&span.text, run);
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
                line.push(&text[copied..i], run);
                // An @agent tag in that agent's own chat colour.
                let paint = agent_tag_paint(t, env.store, name, env.space);
                let pill = Run {
                    color: paint.fg,
                    weight: Some(ty::MEDIUM),
                    ..run
                };
                // A narrow no-break space: the icon stays with `@name`.
                let range = line.push(&format!("{}\u{202f}@{name}", text_icon(icon, "🤖")), pill);
                line.tag(range, paint);
                i += 1 + name.len();
                copied = i;
                continue;
            }
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        line.push(&text[copied..], run);
    }
    line
}

/// One line of message text: selectable, its links pressable.
fn render_line(
    key: String,
    inline: &Inline,
    md: &MdTheme,
    names: &[(String, String)],
    style: TextStyle,
    env: &MdEnv,
) -> El {
    line_el(key, build_line(inline, md, names, env), style, md.ink)
}

fn line_el(key: String, line: Line, style: TextStyle, ink: Color) -> El {
    if line.text.is_empty() {
        // An empty line still takes a line's height (`<br>`).
        return text(" ").key(key).font(style).color(ink);
    }
    let mut el = rich(line.text, line.spans)
        .key(key)
        .font(style)
        .color(ink)
        .selectable(true)
        .span_rings(line.rings)
        .strikes(line.strikes);
    for (range, id) in line.links {
        el = el.link(rune::Link::new(range, id));
    }
    el
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

/// The text as bold (headings, table heads).
fn bold(inline: &Inline) -> Inline {
    inline
        .iter()
        .map(|s| Span {
            bold: true,
            ..s.clone()
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn lines(
    key: &str,
    ix: usize,
    lines: &[Inline],
    md: &MdTheme,
    names: &[(String, String)],
    style: TextStyle,
    env: &MdEnv,
) -> El {
    col().children(
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| render_line(format!("{key}-{ix}-{i}"), l, md, names, style, env)),
    )
}

fn render_block(
    key: &str,
    ix: usize,
    block: &Block,
    md: &MdTheme,
    names: &[(String, String)],
    style: TextStyle,
    env: &MdEnv,
) -> El {
    let t = env.t;
    match block {
        Block::Paragraph(ls) => lines(key, ix, ls, md, names, style, env),
        Block::Heading { level, text } => {
            let size = match level {
                1 => 17.,
                2 => 15.5,
                _ => 14.,
            };
            let heading = TextStyle {
                size,
                weight: ty::BOLD,
                line_height: size * 1.45,
                ..style
            };
            render_line(format!("{key}-{ix}"), &bold(text), md, names, heading, env)
        }
        Block::Rule => roostr_ui::Separator::horizontal().color(t.border).build(t),
        Block::Quote(ls) => {
            let quote = MdTheme {
                ink: t.muted,
                tint: md.tint,
                mention_names: Vec::new(),
            };
            col()
                .border_sides([0.0, 0.0, 0.0, 2.0], t.accent)
                .px(10.)
                .py(2.)
                .child(lines(key, ix, ls, &quote, names, style, env))
        }
        Block::Code { lang, code } => {
            let mono = ty::mono(12.5).lh(12.5 * 1.5);
            let mut boxed = col()
                .fill(t.hover)
                .border(1.0, t.border)
                .radius(radius::MD)
                .px(10.)
                .py(8.);
            if !lang.is_empty() {
                boxed = boxed.child(
                    text(lang.to_uppercase())
                        .font(ty::ui(10.).family(rune::Family::Mono))
                        .color(t.muted)
                        .mb(4.),
                );
            }
            // The box is the theme's, not the bubble's: its text too, or a tinted
            // bubble's dark ink vanishes on a dark theme's box.
            boxed.child(
                col().key(format!("{key}-{ix}-code")).scroll_x().child(
                    text(code.clone())
                        .key(format!("{key}-{ix}-code-text"))
                        .font(mono)
                        .color(t.fg)
                        .selectable(true),
                ),
            )
        }
        Block::List { ordered, items } => {
            let tasky = items.iter().any(|i| i.check.is_some());
            let rows = items.iter().enumerate().map(|(i, item)| {
                let key = format!("{key}-{ix}-li{i}");
                let mut built = build_line(&item.text, md, names, env);
                if tasky {
                    let done = item.check == Some(TaskCheck::Done);
                    let mut r = row().items_start().gap(7.);
                    if item.check.is_some() {
                        r = r.child(
                            roostr_ui::checkbox_display(format!("{key}-check"), done)
                                .small()
                                .build(t)
                                .mt(3.),
                        );
                    }
                    let text = if done {
                        // `.done { opacity: .6; text-decoration: line-through }`.
                        built.strikes = vec![(0, built.text.len(), None)];
                        line_el(key, built, style, md.ink).opacity(0.6)
                    } else {
                        line_el(key, built, style, md.ink)
                    };
                    r.child(col().shrink().child(text))
                } else {
                    let line = line_el(key, built, style, md.ink);
                    let marker = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "•".into()
                    };
                    row()
                        .child(
                            row()
                                .w(20.)
                                .pr(6.)
                                .justify_end()
                                .child(text(marker).font(style).color(md.ink)),
                        )
                        .child(col().shrink().child(line))
                }
            });
            let mut list = col().children(rows);
            if tasky {
                list = list.pl(2.);
            }
            list
        }
        Block::Table(table) => {
            let n = table.head.len().max(1);
            let head_border = roostr_ui::color::mix(t.border, t.fg, 0.5);
            let cell_style = ty::ui(13.).lh(13. * 1.35);
            let rows = table.rows.len();
            let cell = |key: String, inline: &Inline, align: Align, head: bool, last: bool| {
                let theme_for = MdTheme {
                    ink: if head { t.muted } else { md.ink },
                    tint: md.tint,
                    mention_names: Vec::new(),
                };
                let style = if head {
                    TextStyle {
                        weight: ty::SEMIBOLD,
                        ..cell_style
                    }
                } else {
                    cell_style
                };
                let mut line = render_line(
                    key,
                    &if head { bold(inline) } else { inline.clone() },
                    &theme_for,
                    names,
                    style,
                    env,
                );
                let mut c = col().px(8.).py(4.);
                if !last {
                    c = c.border_sides(
                        [0.0, 0.0, 1.0, 0.0],
                        if head { head_border } else { t.border },
                    );
                }
                match align {
                    Align::Right => {
                        line = line.align(rune::Align::End);
                        c = c.items_end();
                    }
                    Align::Center => {
                        line = line.align(rune::Align::Center);
                        c = c.items_center();
                    }
                    _ => {}
                }
                c.child(line)
            };
            let mut grid = el_grid(n);
            for (c, h) in table.head.iter().enumerate() {
                grid = grid.child(cell(
                    format!("{key}-{ix}-h{c}"),
                    h,
                    table.aligns.get(c).copied().unwrap_or_default(),
                    true,
                    false,
                ));
            }
            for (r, cells) in table.rows.iter().enumerate() {
                for (c, value) in cells.iter().enumerate() {
                    grid = grid.child(cell(
                        format!("{key}-{ix}-{r}-{c}"),
                        value,
                        table.aligns.get(c).copied().unwrap_or_default(),
                        false,
                        r + 1 == rows,
                    ));
                }
            }
            col()
                .key(format!("{key}-{ix}-table"))
                .w_full()
                .scroll_x()
                .child(grid)
        }
    }
}

/// `n` equal columns sharing the width (`grid-template-columns: repeat(n, 1fr)`).
fn el_grid(n: usize) -> El {
    rune::el().w_full().equal_columns(n)
}

/// A message body: parsed markdown laid out with the chat's `.md` styles. `origin`
/// (object id, name) leads the body as that object's tag, the same pill an object link in the
/// text gets. `key` scopes the lines' keys.
pub(crate) fn render_markdown(
    key: &str,
    src: &str,
    origin: Option<(&str, &str)>,
    md: &MdTheme,
    env: &MdEnv,
) -> El {
    let mut names = md.mention_names.clone();
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    let mut blocks = parse(src);
    if let Some((object_id, name)) = origin {
        let tag = vec![Span {
            text: name.to_string(),
            link: Some(Link {
                href: format!("/app/object/{object_id}"),
                object_id: Some(object_id.to_string()),
            }),
            ..Span::default()
        }];
        // The first line of the body: a line of its own in the opening paragraph.
        match blocks.first_mut() {
            Some(Block::Paragraph(lines)) => lines.insert(0, tag),
            _ => blocks.insert(0, Block::Paragraph(vec![tag])),
        }
    }
    let style = ty::ui(14.).lh(14. * 1.45);
    let mut out = col().shrink();
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
        let mut el = col()
            .mt(gap)
            .child(render_block(key, ix, block, md, &names, style, env));
        if ix + 1 == count {
            el = el.mb(bottom);
        }
        out = out.child(el);
    }
    out
}
