//! What the data view hands every record view.

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
