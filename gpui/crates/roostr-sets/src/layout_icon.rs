//! Anytype's dataview layout icons (LayoutIcon.svelte): 56×56 currentColor
//! outlines over a subtle plate.

use roostr_ui::{Glyph, glyph};
use rune::{Color, El};

/// The glyph for a `viewType`.
pub(crate) fn layout_glyph(kind: &str) -> Glyph {
    match kind {
        "gallery" => Glyph::LayoutGallery,
        "kanban" => Glyph::LayoutKanban,
        "calendar" => Glyph::LayoutCalendar,
        _ => Glyph::LayoutTable,
    }
}

/// The layout icon at `size` points, tinted by `color`.
pub(crate) fn layout_icon(kind: &str, size: f32, color: Color) -> El {
    glyph(layout_glyph(kind), size).color(color)
}
