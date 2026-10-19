//! Per-space schema/object graph (`routes/app/graph/+page.svelte`,
//! `lib/graph.ts`): types as clusters of their objects on an outer ring, the
//! properties they use on an inner ring, three zoom levels (Types / Schema /
//! Objects), hover and pin highlights, a finder, and `?focus=` pinning.

mod model;
mod paint;
mod panel;
mod scene;
mod text;
mod view;

use gpui_kit::App;

pub use view::GraphView;

/// Start loading the system fonts the graph's world-space labels are laid
/// out with, off the main thread, so the first graph opens without waiting.
pub fn init(_cx: &mut App) {
    text::preload();
}
