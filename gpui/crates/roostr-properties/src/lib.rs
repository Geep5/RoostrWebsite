//! Property values, panes, pickers, type/property pages, credentials and capabilities.

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
mod suggest;
mod type_limit;
mod type_panel;
mod value;
mod widgets;

pub use credential_ui::CredentialCell;
pub use credentials::{
    CredentialTarget, credential_cell_badge, credential_status_badge, credential_status_text,
    reconnect_target,
};
pub use emoji::EmojiPicker;
pub use flow::{FlowEvent, FlowItem, PropertyFlow};
pub use key_picker::KeyPicker;
pub use meta::ObjectMeta;
pub use pane::PropertiesPane;
pub use property_panel::PropertyPanel;
/// Property icon for a relation format (PropIcon).
pub use roostr_core::ui::prop_icon;
pub use suggest::{PropertyPick, PropertySuggest, SuggestEvent, TypeSuggest};
pub use type_panel::TypePanel;
pub use value::PropertyValue;

use gpui_kit::App;

/// Install this crate's window-wide state (credential reconnect/check progress).
/// Call once after `roostr_core::init`.
pub fn init(cx: &mut App) {
    credentials::Progress::init(cx);
}
