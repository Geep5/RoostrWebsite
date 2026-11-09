//! Scroll areas with gpui-kit's scrollbars (`scroll/scrollable.rs`, `gpui_base::Scrollbar` as
//! gpui-kit's theme styles it): a vertical or horizontal Rune scroll viewport with a scroll bar
//! drawn in gpui-kit's look: a `scrollbar_thumb` thumb 6 pt wide that widens to 8 pt and turns
//! `scrollbar_thumb_hover` under the pointer or while dragged, its ends rounded, shown as
//! [`ScrollbarMode`] says.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rune::{Color, Drag, El, Live, Phase, ScrollPos, ScrollSpec, Tx, col, el, row};
use rune_kit::{ScrollBar as _, ScrollBarStyle};

use crate::theme::Theme;

/// gpui-kit's thumb width at rest and under the pointer / dragged.
const THUMB: f32 = 6.0;
const THUMB_ACTIVE: f32 = 8.0;
/// gpui-base's bar: a 16 pt strip along the end edge, the thumb 4 pt in from its outer edge and
/// from the track's ends, and 48 pt long at least.
const STRIP: f32 = 16.0;
const INSET: f32 = 4.0;
const MIN_THUMB: f32 = 48.0;

/// gpui-kit's `ScrollbarMode`: when the scrollbar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarMode {
    /// While scrolling, fading out after it (and while the pointer is on the thumb).
    Scrolling,
    /// While the pointer is over the scroll area, or it scrolls.
    Hover,
    /// Whenever the content overflows, whatever the system's scroll bar preference (Rune's
    /// own bars turn into overlays when macOS shows scroll bars only while scrolling, so this
    /// one is Roostr's, as gpui-base drew it).
    Always,
}

impl ScrollbarMode {
    /// What gpui-kit's `Theme::sync_scrollbar_appearance` picks: `Scrolling` where the system
    /// hides scroll bars at rest (trackpads), else `Hover`.
    pub fn system() -> Self {
        if rune::system_scroll_bars().is_overlay(rune::system_scroller_overlay()) {
            ScrollbarMode::Scrolling
        } else {
            ScrollbarMode::Hover
        }
    }

    /// rune-kit's bar for this mode in gpui-kit's colours and widths. [`ScrollArea`] draws
    /// `Always` itself; this is the kit's nearest look for a bar attached directly.
    pub fn style(self, t: &Theme) -> ScrollBarStyle {
        let (thumb, hover) = (t.kit.scrollbar_thumb, t.kit.scrollbar_thumb_hover);
        let base = match self {
            ScrollbarMode::Scrolling => ScrollBarStyle::menu_scroll(thumb, hover),
            ScrollbarMode::Hover => ScrollBarStyle::scroll_view(&t.rk)
                .thumb(thumb)
                .thumb_hover(hover),
            ScrollbarMode::Always => ScrollBarStyle::from_kit(&t.rk)
                .thumb(thumb)
                .thumb_hover(hover),
        };
        base.width(THUMB).hover_width(THUMB_ACTIVE)
    }
}

impl Default for ScrollbarMode {
    fn default() -> Self {
        Self::system()
    }
}

/// gpui-kit's `overflow_y_scrollbar()` / `overflow_x_scrollbar()`: `content` in a scroll
/// viewport keyed `key` (the area around it is `{key}-scroll-area`) with a scrollbar over its
/// end edge. Size it like the box it replaces (`.h(..)`, `.max_h(..)`, `.grow()`).
pub struct ScrollArea {
    key: String,
    content: El,
    horizontal: bool,
    mode: Option<ScrollbarMode>,
    size: Vec<Box<dyn FnOnce(El) -> El>>,
}

impl ScrollArea {
    /// Scrolls `content` down (`overflow_y_scrollbar`).
    pub fn vertical(key: impl Into<String>, content: El) -> Self {
        Self {
            key: key.into(),
            content,
            horizontal: false,
            mode: None,
            size: Vec::new(),
        }
    }
    /// Scrolls `content` sideways (`overflow_x_scrollbar`).
    pub fn horizontal(key: impl Into<String>, content: El) -> Self {
        Self {
            horizontal: true,
            ..Self::vertical(key, content)
        }
    }
    /// When the bar shows (default: [`ScrollbarMode::system`], as gpui-kit's theme).
    pub fn mode(mut self, mode: ScrollbarMode) -> Self {
        self.mode = Some(mode);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.size.push(Box::new(move |e| e.h(h)));
        self
    }
    pub fn max_h(mut self, h: f32) -> Self {
        self.size.push(Box::new(move |e| e.max_h(h)));
        self
    }
    pub fn w(mut self, w: f32) -> Self {
        self.size.push(Box::new(move |e| e.w(w)));
        self
    }
    pub fn w_full(mut self) -> Self {
        self.size.push(Box::new(|e| e.w_full()));
        self
    }
    /// Takes the free space of a bounded column (`flex_1` with `min_h_0`).
    pub fn grow(mut self) -> Self {
        self.size.push(Box::new(|e| e.grow(1.0).min_h(0.0)));
        self
    }
    /// Hear where the viewport is scrolled to ([`El::on_scroll_offset`] on the viewport).
    pub fn on_scroll_offset(mut self, f: impl Fn(rune::ScrollPos) + 'static) -> Self {
        self.size.push(Box::new(move |e| e.on_scroll_offset(f)));
        self
    }

    pub fn build(self, t: &Theme) -> El {
        let mode = self.mode.unwrap_or_default();
        let mut viewport = if self.horizontal {
            row().key(self.key).scroll_x().child(self.content)
        } else {
            col()
                .key(self.key)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(self.content)
        };
        for f in self.size {
            viewport = f(viewport);
        }
        if mode == ScrollbarMode::Always {
            return always_bar(t, viewport, self.horizontal);
        }
        let style = mode.style(t);
        let style = if self.horizontal {
            style.horizontal()
        } else {
            style
        };
        viewport.scroll_bar_styled(style)
    }
}

/// What an always-shown bar keeps across view runs: where its viewport stands, the pointer
/// over the thumb, a drag, and the live rects it draws from.
struct Bar {
    horizontal: bool,
    pos: Cell<ScrollPos>,
    over_thumb: Cell<bool>,
    dragging: Cell<bool>,
    strip: Live<[f32; 4]>,
    thumb: Live<[f32; 4]>,
    fill: Live<Color>,
    colors: Cell<(Color, Color)>,
}

/// Bars kept per area, as rune-kit keeps its own; cleared once this many pile up.
const KEPT: usize = 256;

thread_local! {
    static BARS: RefCell<HashMap<(u64, String), Rc<Bar>>> = RefCell::new(HashMap::new());
}

fn kept(area: &str, horizontal: bool) -> Rc<Bar> {
    let id = (rune::current_runtime_id(), area.to_owned());
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT && !bars.contains_key(&id) {
            bars.clear();
        }
        bars.entry(id)
            .or_insert_with(|| {
                Rc::new(Bar {
                    horizontal,
                    pos: Cell::new(ScrollPos::default()),
                    over_thumb: Cell::new(false),
                    dragging: Cell::new(false),
                    strip: Live::new([0.0; 4]),
                    thumb: Live::new([0.0; 4]),
                    fill: Live::new(Color::TRANSPARENT),
                    colors: Cell::new((Color::TRANSPARENT, Color::TRANSPARENT)),
                })
            })
            .clone()
    })
}

/// gpui-base's thumb geometry along a viewport `track` long whose content runs `max` past it,
/// scrolled `offset` in: the thumb's drawn start and length, how far its start travels, and
/// its inset from the track's ends.
fn thumb_along(track: f32, max: f32, offset: f32) -> (f32, f32, f32, f32) {
    let logical = (track / (track + max) * track).max(MIN_THUMB).min(track);
    let inset = INSET.clamp(0.0, logical / 2.0);
    let travel = track - logical;
    let at = if max > 0.0 {
        (offset / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (inset + at * travel, logical - 2.0 * inset, travel, inset)
}

impl Bar {
    /// The along-axis index of a point or size: 0 sideways, 1 down.
    fn along(&self) -> usize {
        usize::from(!self.horizontal)
    }

    /// Places the strip and thumb for the viewport's latest position.
    fn place(&self) {
        let at = self.pos.get();
        let a = self.along();
        let (w, h) = (at.viewport[0], at.viewport[1]);
        if at.max[a] <= 0.5 || at.viewport[a] <= 0.0 {
            self.strip.set([0.0; 4]);
            self.thumb.set([0.0; 4]);
            return;
        }
        let active = self.over_thumb.get() || self.dragging.get();
        let width = if active { THUMB_ACTIVE } else { THUMB };
        let (start, len, _, _) = thumb_along(at.viewport[a], at.max[a], at.offset[a]);
        // The thumb hugs the strip's outer edge, INSET in, and widens inward.
        let across = STRIP - INSET - width;
        if self.horizontal {
            self.strip.set([0.0, h - STRIP, w, STRIP]);
            self.thumb.set([start, across, len, width]);
        } else {
            self.strip.set([w - STRIP, 0.0, STRIP, h]);
            self.thumb.set([across, start, width, len]);
        }
        let (rest, hover) = self.colors.get();
        self.fill.set(if active { hover } else { rest });
    }

    /// Dragging the thumb scrolls the viewport keyed `key` to follow it, the thumb held where
    /// it was caught.
    fn drag(&self, key: &str, d: &mut Drag, tx: &mut Tx) {
        tx.live_only();
        let at = self.pos.get();
        let a = self.along();
        let (start, _, travel, inset) = thumb_along(at.viewport[a], at.max[a], at.offset[a]);
        match d.phase {
            Phase::Down => {
                self.dragging.set(true);
                d.data = start;
            }
            Phase::Up => {
                self.dragging.set(false);
                self.place();
                return;
            }
            _ => {}
        }
        if travel <= 0.0 {
            return;
        }
        let moved = if self.horizontal { d.dx } else { d.dy };
        let to = ((d.data + moved - inset) / travel).clamp(0.0, 1.0) * at.max[a];
        if self.horizontal {
            tx.scroll_to_instant(key, to, at.offset[1]);
        } else {
            tx.scroll_to_instant(key, at.offset[0], to);
        }
        self.place();
    }
}

/// gpui-base's `ScrollbarMode::Always` bar on `viewport`: shown whenever the content
/// overflows, whatever macOS's scroll bar preference, along the end edge (the bottom for a
/// sideways viewport) in a 16 pt strip; the thumb takes the hover colour and widens to 8 pt
/// under the pointer or while dragged. Only the thumb takes the pointer, so a wheel over the
/// strip still reaches the content under it (the data table scrolls sideways from there).
fn always_bar(t: &Theme, viewport: El, horizontal: bool) -> El {
    let (area, key, outer, viewport) = viewport.wrap_scroll_area((file!(), line!(), column!()));
    let bar = kept(&area, horizontal);
    bar.colors
        .set((t.kit.scrollbar_thumb, t.kit.scrollbar_thumb_hover));
    let (follow, hover, drag) = (bar.clone(), bar.clone(), bar.clone());
    outer
        .child(viewport.chain_scroll_offset(move |at| {
            follow.pos.set(at);
            follow.place();
        }))
        .child(
            el().key("roostr-scroll-bar")
                .absolute(0.0, 0.0)
                .live_rect(&bar.strip)
                .a11y_hidden()
                .child(
                    el().key("thumb")
                        .absolute(0.0, 0.0)
                        .live_rect(&bar.thumb)
                        .radius(THUMB / 2.0)
                        .live_fill(&bar.fill)
                        .on_drag_any(move |d, tx| drag.drag(&key, d, tx))
                        .on_hover_change_any(move |inside, tx| {
                            tx.live_only();
                            hover.over_thumb.set(inside);
                            hover.place();
                        }),
                ),
        )
}
