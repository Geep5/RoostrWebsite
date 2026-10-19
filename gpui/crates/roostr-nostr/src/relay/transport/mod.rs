//! The socket under a [`super::Relay`]: a thread running `async-tungstenite`
//! over `async-net` + rustls on native, `web_sys::WebSocket` in the browser.
//! Both expose the same `Conn`: `dial`, `send` (queued until open), `close`.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub(super) use native::Conn;
#[cfg(target_family = "wasm")]
pub(super) use web::Conn;
