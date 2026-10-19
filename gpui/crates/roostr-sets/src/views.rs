//! What the data view hands every record view, and what views tell it back.

use serde_json::Value as Json;

use crate::config::SortRule;

/// A record view's inputs: the query, the view's sorts, and the view
/// object's `updatedAt` (gallery/kanban/calendar reload when it moves).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ViewInput {
    pub body: Option<Json>,
    pub sorts: Vec<SortRule>,
    pub updated_at: i64,
}

/// What a record view tells the data view.
pub(crate) enum ViewMsg {
    /// Records changed (the web's `onchanged`).
    Changed,
}
