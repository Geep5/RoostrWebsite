//! Property values, panes, pickers, type/property pages, credentials and capabilities.
//!
//! Every view draws with the host's theme and overlay layer (`s, cx, t, layer`); per-instance
//! UI state lives in this crate's slots, keyed by the caller's key or the object id. The app
//! calls [`register`] once.

mod calendar;
mod capability;
mod credential_ui;
mod credentials;
mod emoji;
mod flow;
mod key_picker;
mod meta;
mod option_picker;
mod pane;
mod project_folder;
mod property_panel;
mod repeat_view;
mod role;
mod suggest;
mod type_limit;
mod type_panel;
pub mod value;
mod widgets;

pub use credential_ui::CredentialCell;
pub use credentials::{
    CredentialTarget, credential_cell_badge, credential_status_badge, credential_status_text,
    reconnect_target,
};
pub use emoji::EmojiPicker;
pub use flow::{FlowEvent, FlowItem, PropertyFlow, PropertyFlowState};
pub use key_picker::KeyPicker;
pub use meta::{object_meta, open_meta};
pub use pane::{open_pane, properties_pane};
pub use property_panel::{open_property_panel, property_panel};
/// Property icon for a relation format (PropIcon).
pub use roostr_ui::prop_icon;
pub use suggest::{
    PropertyPick, PropertySuggest, PropertySuggestState, SuggestEvent, TypeSuggest,
    TypeSuggestState,
};
pub use type_panel::{open_type_panel, type_panel};
pub use value::{PropertyValue, commit_pending};

use roostr_core::State;

// In-crate tests (views that are `pub(crate)`) use the same headless support as `tests/`,
// which names this crate `roostr_properties`.
#[cfg(test)]
extern crate self as roostr_properties;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod test_support;

/// Installs this crate's listeners (replaces GPUI's `init`). Call once after `State::new`.
pub fn register(s: &mut State) {
    value::register(s);
    repeat_view::register(s);
    meta::register(s);
    property_panel::register(s);
    pane::register(s);
}

/// Drops every per-object state this crate keeps for `object_id` (GPUI
/// dropped the view entities when the page closed). Optional: the views
/// reopen themselves on their next draw.
pub fn forget(s: &mut State, object_id: &str) {
    credential_ui::forget(s, object_id);
    capability::forget(s, object_id);
    repeat_view::forget(s, object_id);
    project_folder::forget(s, object_id);
    meta::forget(s, object_id);
    type_panel::forget(s, object_id);
    property_panel::forget(s, object_id);
    pane::forget(s, object_id);
}
