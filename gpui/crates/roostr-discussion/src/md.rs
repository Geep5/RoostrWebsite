//! Chat message markdown (`renderMarkdown` + the Discussion's `.md` styles and
//! `mentionPills`): `roostr_core::markdown` parses, this lays the blocks out
//! and paints inline styles, links, object links and @agent mention pills.
//! Every line takes part in the window text selection, as the web view's DOM
//! text did: a drag selects across lines and messages, ⌘C copies, and on a
//! touch screen a long press selects a word with handles and a Copy menu.

use std::cell::Cell;
use std::ops::Range;

use gpui_kit::base::{
    TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun, Theme,
    TouchHandleLayout,
};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::separator::Separator;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::markdown::{Align, Block, Inline, Link, Span, TaskCheck, parse};
use roostr_core::nav::{self, Route};
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::options::{TagPaint, tint_paint};
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

enum Click {
    Object(String),
    Url(String),
}

/// One rendered line: text, runs, clickable ranges and the ranges drawn as
/// tags (a kit-tag box painted behind the text, see [`SelectableLine`]).
struct Line {
    text: String,
    runs: Vec<TextRun>,
    clicks: Vec<(Range<usize>, Click)>,
    tags: Vec<(Range<usize>, TagPaint)>,
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
        tags: Vec::new(),
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
                    // The composer's !object tag: the kit tag look in green
                    // (a coloured bubble keeps its own tint).
                    let paint = match md.tint {
                        Some(tint) => {
                            run.background_color = Some(tint);
                            run.color = md.ink;
                            None
                        }
                        None => {
                            let paint = tint_paint(rgb(crate::discussion::OBJECT_GREEN).into());
                            run.color = paint.fg;
                            Some(paint)
                        }
                    };
                    // A narrow no-break space: the icon stays with the name.
                    line.push(&format!("{}\u{202f}", text_icon(&icon, &type_icon)), &run);
                    line.push(&span.text, &run);
                    if let Some(paint) = paint {
                        line.tags.push((start..line.text.len(), paint));
                    }
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
                // The composer's @agent tag: the kit tag look in the person's
                // chat colour, its box painted behind the text.
                let chat = Store::global(cx).read(cx).chat_color.clone();
                let paint = tint_paint(roostr_core::color::hex(&chat).unwrap_or(theme::accent()));
                let pill = TextRun {
                    color: paint.fg,
                    font: Font {
                        weight: FontWeight::MEDIUM,
                        ..run.font.clone()
                    },
                    ..run.clone()
                };
                let start = line.text.len();
                // A narrow no-break space: the icon stays with `@name`.
                line.push(&format!("{}\u{202f}@{name}", text_icon(icon, "🤖")), &pill);
                line.tags.push((start..line.text.len(), paint));
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
    order: &Cell<u64>,
    cx: &App,
) -> AnyElement {
    let line = build_line(inline, md, names, cx);
    if line.text.is_empty() {
        // An empty line still takes a line's height (`<br>`).
        return div().child(" ").into_any_element();
    }
    let text = SharedString::from(line.text);
    let styled = StyledText::new(text.clone()).with_runs(line.runs);
    let layout = styled.layout().clone();
    let element = if line.clicks.is_empty() {
        styled.into_any_element()
    } else {
        let (ranges, targets): (Vec<Range<usize>>, Vec<Click>) = line.clicks.into_iter().unzip();
        InteractiveText::new(id.clone(), styled)
            .on_click(ranges, move |ix, _, cx| match &targets[ix] {
                Click::Object(id) => nav::navigate(Route::Object(id.clone()), cx),
                Click::Url(url) => cx.open_url(url),
            })
            .into_any_element()
    };
    SelectableLine::new(id, text, layout, element, order)
        .tags(line.tags)
        .into_any_element()
}

/// One laid-out line of message text in the window text selection: gpui-base's
/// `SelectableText`, for text with styled runs and links (the line's own
/// `StyledText` or `InteractiveText`, which it lays out and paints).
struct SelectableLine {
    id: ElementId,
    text: SharedString,
    layout: TextLayout,
    element: AnyElement,
    /// Reading order among the lines of the conversation.
    order: u64,
    /// Ranges painted as tags behind the text.
    tags: Vec<(Range<usize>, TagPaint)>,
}

impl SelectableLine {
    /// `element` is laid out as `layout`, showing `text`; the line takes the
    /// next reading position from `order`.
    fn new(
        id: ElementId,
        text: SharedString,
        layout: TextLayout,
        element: AnyElement,
        order: &Cell<u64>,
    ) -> Self {
        let this = Self {
            id,
            text,
            layout,
            element,
            order: order.get(),
            tags: Vec::new(),
        };
        order.set(order.get() + 1);
        this
    }

    fn tags(mut self, tags: Vec<(Range<usize>, TagPaint)>) -> Self {
        self.tags = tags;
        self
    }
}

impl IntoElement for SelectableLine {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableLine {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = (Hitbox, TouchHandleLayout);

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            global_id.expect("a selectable line has an id"),
            |retained: Option<TextSelectionHandle>, _| {
                let handle =
                    retained.unwrap_or_else(|| TextSelectionHandle::new(self.text.clone(), cx));
                (handle.clone(), handle)
            },
        );
        handle.set_fallback_copy_text(self.text.clone(), cx);
        (self.element.request_layout(window, cx), handle)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element.prepaint(window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let handles = handle.prepaint_touch_handles(window, cx);
        (hitbox, handles)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        (hitbox, handles): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected_before = TextSelection::selected_text(window, cx);
        let projection = handle.update_runs(
            &[
                TextSelectionRun::new(self.text.clone(), self.layout.clone(), bounds)
                    .with_document_order(self.order),
            ],
            cx,
        );
        if selected_before != TextSelection::selected_text(window, cx) {
            window.refresh();
        }
        let line_height = self.layout.line_height();
        // Tags: the kit tag box (fill, 1px border, radius 4) behind each
        // range, one box per row a wrapped tag covers, 2px past the glyphs.
        for (range, paint) in &self.tags {
            let (Some(mut start), Some(end)) = (
                self.layout.position_for_index(range.start),
                self.layout.position_for_index(range.end),
            ) else {
                continue;
            };
            // A tag the line wraps right before reports its start at the end
            // of the previous row: it really starts at the next row's edge.
            let first = self.text[range.start..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
            if let Some(after) = self.layout.position_for_index(range.start + first)
                && after.y > start.y
            {
                start = point(bounds.left(), after.y);
            }
            for row in selection_quads(start, end, bounds, line_height) {
                let row = Bounds::from_corners(
                    point(row.left() - px(2.), row.top() + px(1.)),
                    point(row.right() + px(2.), row.bottom() - px(1.)),
                );
                window.paint_quad(quad(
                    row,
                    px(4.),
                    paint.bg,
                    px(1.),
                    paint.border,
                    BorderStyle::Solid,
                ));
            }
        }
        let color = Theme::global(cx).tokens.colors.selection;
        let mut edges = None;
        for range in projection.ranges().iter().flatten() {
            let (Some(start), Some(end)) = (
                self.layout.position_for_index(range.start),
                self.layout.position_for_index(range.end),
            ) else {
                continue;
            };
            for quad in selection_quads(start, end, bounds, line_height) {
                window.paint_quad(fill(quad, color));
            }
            let caret = |at: Point<Pixels>| Bounds::new(at, size(px(0.), line_height));
            edges = Some((caret(start), caret(end)));
        }
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.order)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        let registration = match edges {
            Some((start, end)) => registration.with_selection_edges(start, end),
            None => registration,
        };
        handle.register(registration, window, cx);
        // Links set the pointing hand over themselves as they paint.
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        self.element.paint(window, cx);
        handle.paint_touch_handles(handles, theme::accent(), window, cx);
    }
}

/// The highlight of `start..end` on a wrapped line: the first row from the
/// start, full middle rows, the last row to the end.
fn selection_quads(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(
            start,
            point(end.x, end.y + line_height),
        )];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        point(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            point(bounds.left(), start.y + line_height),
            point(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        point(bounds.left(), end.y),
        point(end.x, end.y + line_height),
    ));
    quads
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
    order: &Cell<u64>,
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
                order,
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
    order: &Cell<u64>,
    cx: &App,
) -> AnyElement {
    match block {
        Block::Paragraph(ls) => lines(id, ix, ls, md, names, order, cx).into_any_element(),
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
                order,
                cx,
            ))
            .into_any_element(),
        Block::Rule => Separator::horizontal()
            .color(theme::border())
            .into_any_element(),
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
                .child(lines(id, ix, ls, &quote, names, order, cx))
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
            .child(div().whitespace_nowrap().child({
                let code = SharedString::from(code.clone());
                let styled = StyledText::new(code.clone());
                let layout = styled.layout().clone();
                SelectableLine::new(
                    ElementId::Name(format!("{id}-{ix}-code-text").into()),
                    code,
                    layout,
                    styled.into_any_element(),
                    order,
                )
            }))
            .into_any_element(),
        Block::List { ordered, items } => {
            let tasky = items.iter().any(|i| i.check.is_some());
            let rows = items.iter().enumerate().map(|(i, item)| {
                let text = render_line(
                    ElementId::Name(format!("{id}-{ix}-li{i}").into()),
                    &item.text,
                    md,
                    names,
                    order,
                    cx,
                );
                if tasky {
                    let done = item.check == Some(TaskCheck::Done);
                    let check = roostr_core::ui::checkbox_display(
                        ElementId::Name(format!("{id}-{ix}-check{i}").into()),
                        done,
                    )
                    .small()
                    .flex_none()
                    .mt(px(3.));
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
                        order,
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
        .map(|s| Span {
            bold: true,
            ..s.clone()
        })
        .collect()
}

/// A message body: parsed markdown laid out with the chat's `.md` styles.
/// Its lines take part in the window text selection in reading order from
/// `order` (one message's lines after the previous message's). `origin`
/// (object id, name) leads the body as that object's tag, the same pill an
/// object link in the text gets.
pub(crate) fn render_markdown(
    id: impl Into<ElementId>,
    src: &str,
    origin: Option<(&str, &str)>,
    md: &MdTheme,
    order: u64,
    cx: &App,
) -> AnyElement {
    let id: ElementId = id.into();
    let key = format!("{id:?}");
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
    let order = Cell::new(order);
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
            .child(render_block(&key, ix, block, md, &names, &order, cx));
        if ix + 1 == count {
            el = el.mb(px(bottom));
        }
        out = out.child(el);
    }
    out.into_any_element()
}
