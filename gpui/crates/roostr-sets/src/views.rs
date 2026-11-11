//! What the data view hands every record view.

use serde_json::Value as Json;

use crate::config::SortRule;

/// A record view's inputs: the query, the view's sorts, the collection's
/// member order while no view sort overrides it, and the view object's
/// `updatedAt` (gallery/kanban/calendar reload when it moves).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ViewInput {
    pub body: Option<Json>,
    pub sorts: Vec<SortRule>,
    /// A collection without view sorts: its rows show in this (member) order.
    pub members: Option<Vec<String>>,
    pub updated_at: i64,
}

impl ViewInput {
    /// Put `rows` in the collection's member order, when it orders them.
    pub(crate) fn order(&self, rows: &mut [roostr_core::QueryRow]) {
        if let Some(members) = &self.members {
            roostr_core::collection::sort_by_members(rows, members);
        }
    }
}
