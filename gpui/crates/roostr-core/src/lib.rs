//! Roostr's model, free of any UI framework: wire types, the backend
//! abstraction and daemon/harness clients, and the app [`State`] (store,
//! navigation, pane and UI preferences) whose async work runs on
//! [`roostr_rt`]. Drawing lives in `roostr-ui` and the feature crates.

pub mod account;
pub mod api;
pub mod backend;
pub mod blossom;
pub mod color;
pub mod cover;
pub mod create;
#[cfg(not(target_family = "wasm"))]
pub mod daemon;
pub mod dates;
pub mod devices;
pub mod filters;
pub mod harness;
pub mod http;
pub mod icons;
pub mod invite;
pub mod io;
pub mod layout;
pub mod lite;
pub mod lite_ui;
pub mod markdown;
pub mod mode;
pub mod nav;
pub mod objects;
pub mod options;
pub mod pane;
pub mod pins;
pub mod profile;
pub mod protected;
pub mod recent;
pub mod relations;
pub mod repeat;
pub mod serving;
mod state;
pub mod store;
pub mod sync_status;
pub mod sync_trace;
pub mod types;
pub mod ui_state;
pub mod view_seeds;
#[cfg(target_family = "wasm")]
pub mod web;

pub use backend::{Backend, BackendEvent, JsonMap};
pub use icons::{Assets, Glyph};
pub use nav::{NavEvent, Navigator, Route};
pub use pane::{PaneState, PaneTab};
pub use state::{Cx, State};
pub use store::{AgentRef, Connection, ObjectSlot, Store, StoreEvent, TypeDef};
pub use types::*;
pub use ui_state::UiState;
