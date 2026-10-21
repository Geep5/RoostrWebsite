//! Shared foundation of the Roostr GPUI app: wire types, the backend
//! abstraction and daemon/harness clients, the global [`Store`], navigation,
//! UI state, and the design system every feature crate builds on.

pub mod account;
pub mod api;
pub mod backend;
pub mod color;
pub mod create;
#[cfg(not(target_family = "wasm"))]
pub mod daemon;
pub mod dates;
#[cfg(not(any(target_family = "wasm", target_os = "ios")))]
pub mod dev;
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
pub mod motion;
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
pub mod store;
pub mod sync_status;
pub mod theme;
pub mod types;
pub mod ui;
pub mod ui_state;
pub mod view_seeds;
#[cfg(target_family = "wasm")]
pub mod web;

use std::sync::Arc;

use gpui_kit::App;

pub use backend::{Backend, BackendEvent, JsonMap};
pub use icons::{Assets, Glyph};
pub use nav::{NavEvent, Navigator, Route};
pub use pane::PaneState;
pub use store::{AgentRef, Connection, ObjectSlot, Store, StoreEvent, TypeDef};
pub use types::*;
pub use ui_state::UiState;

/// Install every core global. Call once after `gpui_kit::init(cx)`.
/// `harness` is the computer's agent harness, when the backend is a local daemon.
/// In the browser the application already carries the page's fetch client
/// (`Application::with_http_client`).
pub fn init(backend: Arc<dyn Backend>, harness: Option<Arc<harness::Harness>>, cx: &mut App) {
    #[cfg(not(target_family = "wasm"))]
    set_image_client(cx);
    theme::init(cx);
    UiState::init(cx);
    cx.set_global(harness::GlobalHarness(harness));
    Navigator::init(cx);
    PaneState::init(cx);
    Store::init(backend, cx);
    profile::init(cx);
}

/// Remote images (`img("https://…")`: favicons, icons, avatars) load through GPUI's HTTP client.
/// Some hosts (Wikimedia) refuse bare user agents; name the app and a contact URL.
#[cfg(not(target_family = "wasm"))]
fn set_image_client(cx: &mut App) {
    match reqwest_client::ReqwestClient::user_agent(concat!(
        "Roostr/",
        env!("CARGO_PKG_VERSION"),
        " (+https://roostr.space)"
    )) {
        Ok(client) => cx.set_http_client(Arc::new(client)),
        Err(error) => log::warn!("no HTTP client for remote images: {error:#}"),
    }
}
