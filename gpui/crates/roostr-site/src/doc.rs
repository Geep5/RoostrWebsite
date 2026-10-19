//! The dark document pages, `/guide` and `/privacy`: a 720px column of
//! headings, paragraphs and lists, scrolled like a browser body (keyboard
//! included), with in-page anchors for the guide's table of contents.

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;
use roostr_core::theme::fonts;

use crate::scroll_keys;
use crate::text::{Rich, RichStyle, home};

/// `body` text of the site pages.
pub(crate) fn page_fg() -> Hsla {
    rgb(0xf5f5f7).into()
}
/// `p, li`.
fn body_fg() -> Hsla {
    rgb(0xd6d6db).into()
}
/// `.home`, `.updated`, `.foot`.
fn dim() -> Hsla {
    rgb(0x9a9aa2).into()
}
/// `a`.
fn link() -> Hsla {
    rgb(0x8ab4ff).into()
}

/// `b`, `a`, `code` colours.
fn rich() -> RichStyle {
    RichStyle {
        bold: Some(page_fg()),
        link: link(),
        code_bg: rgb(0x2a2a2e).into(),
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

pub(crate) struct DocPage {
    blocks: &'static [Block],
    style: DocStyle,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

/// `line-height: 1.6` everywhere on these pages.
const LH: f32 = 1.6;

impl DocPage {
    pub(crate) fn new(
        blocks: &'static [Block],
        style: DocStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            blocks,
            style,
            scroll: ScrollHandle::new(),
            focus,
        }
    }

    /// Follow `#anchor`: put that heading at the top, less its 16px
    /// `scroll-margin-top`.
    fn jump(&mut self, anchor: &str, cx: &mut Context<Self>) {
        let Some(ix) = self
            .blocks
            .iter()
            .position(|b| matches!(b, Block::H2(Some(id), _) if *id == anchor))
        else {
            return;
        };
        let Some(bounds) = self.scroll.bounds_for_item(ix) else {
            return;
        };
        let offset = self.scroll.offset();
        let top = bounds.top() - self.scroll.bounds().top() - offset.y;
        let max = self.scroll.max_offset().y;
        let y = (top - px(16.)).max(px(0.)).min(max);
        self.scroll.set_offset(point(offset.x, -y));
        cx.notify();
    }

    /// (margin-top, margin-bottom) of a block, before collapsing.
    fn margins(&self, block: &Block) -> (f32, f32) {
        match block {
            Block::Home | Block::Toc(_) => (0., 0.),
            Block::Title(_) => (24., 4.),
            Block::Updated(_) => (0., self.style.updated_margin),
            Block::H2(..) => self.style.h2_margin,
            Block::H3(_) => (22., 4.),
            // `<p>`/`<ul>`/`<ol>`: 1em; the first/last `li` margins collapse into them.
            Block::P(_) | Block::Ul(_) | Block::Ol(_) => (16., 16.),
            Block::Foot(_) => (48., 14.),
        }
    }

    fn list(&self, ix: usize, items: &'static [&'static str], ordered: bool) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(self.style.li_margin))
            .children(items.iter().enumerate().map(|(n, item)| {
                let marker = if ordered {
                    format!("{}.", n + 1)
                } else {
                    "•".to_string()
                };
                div()
                    .flex()
                    .child(
                        div()
                            .flex_none()
                            .w(px(40.))
                            .pr(px(if ordered { 6. } else { 9. }))
                            .flex()
                            .justify_end()
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Rich::markup(item, rich()).into_element(("li", ix * 1000 + n))),
                    )
            }))
    }

    fn block(&self, ix: usize, block: &Block, cx: &mut Context<Self>) -> AnyElement {
        match block {
            Block::Home => div()
                .h(px(16. * LH))
                .flex()
                .items_center()
                .child(
                    div()
                        .id("home")
                        .cursor_pointer()
                        .text_size(px(14.))
                        .text_color(dim())
                        .child("← Roostr")
                        .on_click(|_, _, cx| home().open(cx)),
                )
                .into_any_element(),
            Block::Title(text) => div()
                .text_size(px(34.))
                .line_height(relative(LH))
                .font_weight(FontWeight::BOLD)
                .child(*text)
                .into_any_element(),
            Block::Updated(text) => div()
                .text_size(px(self.style.updated_size))
                .text_color(dim())
                .child(*text)
                .into_any_element(),
            Block::Toc(entries) => div()
                .flex()
                .flex_wrap()
                .gap_x(px(14.))
                .gap_y(px(6.))
                .pt(px(12.))
                .pb(px(4.))
                .border_t_1()
                .border_b_1()
                .border_color(rgb(0x333338))
                .text_size(px(14.))
                .text_color(link())
                .children(entries.iter().map(|(anchor, label)| {
                    let anchor: &'static str = anchor;
                    div()
                        .id(anchor)
                        .cursor_pointer()
                        .child(*label)
                        .on_click(cx.listener(move |this, _, _, cx| this.jump(anchor, cx)))
                }))
                .into_any_element(),
            Block::H2(_, text) => div()
                .text_size(px(self.style.h2_size))
                .font_weight(FontWeight::BOLD)
                .child(*text)
                .into_any_element(),
            Block::H3(text) => div()
                .text_size(px(16.))
                .font_weight(FontWeight::BOLD)
                .child(*text)
                .into_any_element(),
            Block::P(markup) => div()
                .text_color(body_fg())
                .child(Rich::markup(markup, rich()).into_element(("p", ix)))
                .into_any_element(),
            Block::Ul(items) => self
                .list(ix, items, false)
                .text_color(body_fg())
                .into_any_element(),
            Block::Ol(items) => self
                .list(ix, items, true)
                .text_color(body_fg())
                .into_any_element(),
            Block::Foot(markup) => div()
                .text_size(px(14.))
                .text_color(dim())
                .child(Rich::markup(markup, rich()).into_element(("foot", ix)))
                .into_any_element(),
        }
    }
}

impl Focusable for DocPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DocPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut previous_bottom = 0.;
        let mut children = Vec::with_capacity(self.blocks.len());
        for (ix, block) in self.blocks.iter().enumerate() {
            let (top, bottom) = self.margins(block);
            // Adjacent vertical margins collapse to the larger one.
            let gap = if ix == 0 {
                top
            } else {
                top.max(previous_bottom)
            };
            previous_bottom = bottom;
            children.push(
                div().flex().justify_center().w_full().mt(px(gap)).child(
                    div()
                        .w_full()
                        .max_w(px(720.))
                        .px(px(24.))
                        .child(self.block(ix, block, cx)),
                ),
            );
        }
        let scroll = self.scroll.clone();
        scroll_keys(div().id("site-doc"), &self.focus, cx, |this: &Self| {
            &this.scroll
        })
        .relative()
        .size_full()
        .bg(roostr_core::theme::bg())
        .text_color(page_fg())
        .font_family(fonts::UI)
        .text_size(px(16.))
        .line_height(relative(LH))
        .child(
            div()
                .id("site-doc-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .pt(px(48.))
                .pb(px(96. + previous_bottom))
                .children(children),
        )
        .vertical_scrollbar(&scroll)
    }
}
