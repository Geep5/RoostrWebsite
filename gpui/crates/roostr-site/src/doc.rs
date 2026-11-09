//! The dark document pages, `/guide` and `/privacy`: a 720 pt column of headings, paragraphs
//! and lists, scrolled like a browser body (keyboard included), with in-page anchors for the
//! guide's table of contents.

use roostr_core::State;
use roostr_ui::ty::Weights as _;
use roostr_ui::{ScrollArea, Theme, color::rgb, ty};
use rune::{Color, Cursor, El, TextStyle, Tx, col, row, text};

use crate::text::{Rich, RichStyle, home};
use crate::{DOC_SCROLL, scroll_root, track_scroll};

/// `body` text of the site pages.
pub(crate) fn page_fg() -> Color {
    rgb(0xf5f5f7)
}
/// `p, li`.
fn body_fg() -> Color {
    rgb(0xd6d6db)
}
/// `.home`, `.updated`, `.foot`.
fn dim() -> Color {
    rgb(0x9a9aa2)
}
/// `a`.
fn link() -> Color {
    rgb(0x8ab4ff)
}

/// `b`, `a`, `code` colours.
fn rich() -> RichStyle {
    RichStyle {
        bold: Some(page_fg()),
        link: link(),
        code_bg: rgb(0x2a2a2e),
    }
}

/// One block of a document page.
pub(crate) enum Block {
    /// `<a class="home" href="/">← Roostr</a>`.
    Home,
    /// `<h1>`.
    Title(&'static str),
    /// `<p class="updated">`.
    Updated(&'static str),
    /// `<nav class="toc">`: (anchor id, label).
    Toc(&'static [(&'static str, &'static str)]),
    /// `<h2 id>`.
    H2(Option<&'static str>, &'static str),
    /// `<h3>`.
    H3(&'static str),
    /// `<p>` (markup, see [`Rich::markup`]).
    P(&'static str),
    /// `<ul>` of markup items.
    Ul(&'static [&'static str]),
    /// `<ol>` of markup items.
    Ol(&'static [&'static str]),
    /// `<p class="foot">`.
    Foot(&'static str),
}

/// What differs between the two pages' stylesheets.
pub(crate) struct DocStyle {
    pub h2_size: f32,
    /// `h2 { margin: top 0 bottom }`.
    pub h2_margin: (f32, f32),
    pub updated_size: f32,
    pub updated_margin: f32,
    /// `li { margin: N 0 }`.
    pub li_margin: f32,
}

/// `line-height: 1.6` everywhere on these pages.
const LH: f32 = 1.6;

/// The page's text at `size`: the UI font, `line-height: 1.6`.
fn font(size: f32) -> TextStyle {
    ty::ui(size).lh(size * LH)
}

/// The element key of the heading `#anchor` jumps to.
pub(crate) fn anchor_key(anchor: &str) -> String {
    format!("site-anchor-{anchor}")
}

/// (margin-top, margin-bottom) of a block, before collapsing.
fn margins(style: &DocStyle, block: &Block) -> (f32, f32) {
    match block {
        Block::Home | Block::Toc(_) => (0., 0.),
        Block::Title(_) => (24., 4.),
        Block::Updated(_) => (0., style.updated_margin),
        Block::H2(..) => style.h2_margin,
        Block::H3(_) => (22., 4.),
        // `<p>`/`<ul>`/`<ol>`: 1em; the first/last `li` margins collapse into them.
        Block::P(_) | Block::Ul(_) | Block::Ol(_) => (16., 16.),
        Block::Foot(_) => (48., 14.),
    }
}

fn list(style: &DocStyle, items: &'static [&'static str], ordered: bool) -> El {
    col()
        .w_full()
        .gap(style.li_margin)
        .children(items.iter().enumerate().map(|(n, item)| {
            let marker = if ordered {
                format!("{}.", n + 1)
            } else {
                "•".to_string()
            };
            row()
                .w_full()
                .items_start()
                .child(
                    row()
                        .w(40.)
                        .pr(if ordered { 6. } else { 9. })
                        .justify_end()
                        .child(text(marker).font(font(16.)).color(body_fg())),
                )
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(Rich::markup(item, rich()).build(font(16.), body_fg())),
                )
        }))
}

/// A plain wrapping line of the page.
fn line(s: &'static str, font: TextStyle, color: Color) -> El {
    text(s).font(font).color(color).wrap()
}

fn block(style: &DocStyle, block: &Block) -> El {
    match block {
        Block::Home => row().h(16. * LH).items_center().child(
            text("← Roostr")
                .font(font(14.))
                .color(dim())
                .cursor(Cursor::Pointer)
                .on_press_tx(|s: &mut State, tx: &mut Tx| home().open(s, tx)),
        ),
        Block::Title(s) => line(s, font(34.).bold(), page_fg()),
        Block::Updated(s) => line(s, font(style.updated_size), dim()),
        Block::Toc(entries) => row()
            .w_full()
            .flex_wrap()
            .gaps(6., 14.)
            .pt(12.)
            .pb(4.)
            .border_sides([1., 0., 1., 0.], rgb(0x333338))
            .border_layout()
            .children(entries.iter().map(|&(anchor, label)| {
                text(label)
                    .font(font(14.))
                    .color(link())
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                        // `scroll-margin-top: 16px`.
                        tx.scroll_key_to(anchor_key(anchor), 16.);
                    })
            })),
        Block::H2(_, s) => line(s, font(style.h2_size).bold(), page_fg()),
        Block::H3(s) => line(s, font(16.).bold(), page_fg()),
        Block::P(markup) => Rich::markup(markup, rich()).build(font(16.), body_fg()),
        Block::Ul(items) => list(style, items, false),
        Block::Ol(items) => list(style, items, true),
        Block::Foot(markup) => Rich::markup(markup, rich()).build(font(14.), dim()),
    }
}

/// A document page: `blocks` in `style`, filling its parent.
pub(crate) fn view(s: &State, blocks: &'static [Block], style: &DocStyle) -> El {
    let mut previous_bottom = 0.;
    let mut children = Vec::with_capacity(blocks.len());
    for (ix, b) in blocks.iter().enumerate() {
        let (top, bottom) = margins(style, b);
        // Adjacent vertical margins collapse to the larger one.
        let gap = if ix == 0 {
            top
        } else {
            top.max(previous_bottom)
        };
        previous_bottom = bottom;
        let mut wrapper = row()
            .w_full()
            .justify_center()
            .mt(gap)
            .child(col().w_full().max_w(720.).px(24.).child(block(style, b)));
        if let Block::H2(Some(anchor), _) = b {
            wrapper = wrapper.key(anchor_key(anchor));
        }
        children.push(wrapper);
    }
    let content = col()
        .w_full()
        .pt(48.)
        .pb(96. + previous_bottom)
        .children(children);
    let viewport = track_scroll(s, ScrollArea::vertical(DOC_SCROLL, content).grow().w_full())
        .build(Theme::dark());
    scroll_root(
        col()
            .key("site-doc")
            .w_full()
            .h_full()
            .fill(Theme::dark().bg)
            .child(viewport),
    )
}
