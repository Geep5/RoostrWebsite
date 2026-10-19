//! Anytype's dataview layout icons (LayoutIcon.svelte): 56×56 currentColor
//! outlines over a subtle plate.

use gpui_kit::component::Icon;
use gpui_kit::*;
use roostr_core::Glyph;

/// The glyph for a `viewType`.
pub(crate) fn layout_glyph(kind: &str) -> Glyph {
    match kind {
        "gallery" => Glyph::LayoutGallery,
        "kanban" => Glyph::LayoutKanban,
        "calendar" => Glyph::LayoutCalendar,
        _ => Glyph::LayoutTable,
    }
}

/// The layout icon at `size` px, tinted by `color`.
pub(crate) fn layout_icon(kind: &str, size: f32, color: Hsla) -> Icon {
    Icon::new(layout_glyph(kind))
        .size(px(size))
        .text_color(color)
}
