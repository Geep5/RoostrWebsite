//! Per-space schema/object graph (`routes/app/graph/+page.svelte`, `lib/graph.ts`): types as
//! clusters of their objects on an outer ring, the properties they use on an inner ring, three
//! zoom levels (Types / Schema / Objects), hover and pin highlights, a finder, `?focus=`
//! pinning, and this computer's engine and harness live on the graph.

mod geom;
mod labels;
mod live;
mod model;
mod panel;
mod scene;
mod view;

pub use view::{Graphs, register, view};

#[cfg(test)]
mod tests;
