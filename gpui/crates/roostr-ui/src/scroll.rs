//! Scroll areas with gpui-kit's scrollbars (`scroll/scrollable.rs`, `gpui_base::Scrollbar` as
//! gpui-kit's theme styles it): a vertical or horizontal Rune scroll viewport with rune-kit's
//! scroll bar drawn in gpui-kit's look: a `scrollbar_thumb` thumb 6 pt wide that widens to 8 pt
//! and turns `scrollbar_thumb_hover` under the pointer or while dragged, its ends rounded, shown
//! as [`ScrollbarMode`] says.

use rune::{El, ScrollSpec, col, row};
use rune_kit::{ScrollBar as _, ScrollBarStyle};

use crate::theme::Theme;

/// gpui-kit's thumb width at rest and under the pointer / dragged.
const THUMB: f32 = 6.0;
const THUMB_ACTIVE: f32 = 8.0;

/// gpui-kit's `ScrollbarMode`: when the scrollbar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarMode {
    /// While scrolling, fading out after it (and while the pointer is on the thumb).
    Scrolling,
    /// While the pointer is over the scroll area, or it scrolls.
    Hover,
    /// Whenever the content overflows.
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

    /// rune-kit's bar for this mode in gpui-kit's colours and widths.
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

    pub fn build(self, t: &Theme) -> El {
        let style = self.mode.unwrap_or_default().style(t);
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
        let style = if self.horizontal {
            style.horizontal()
        } else {
            style
        };
        viewport.scroll_bar_styled(style)
    }
}
