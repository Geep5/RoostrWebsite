//! Roostr on iPhone and iPad: the desktop app's Rune views (`roostr-app` in the
//! `roostr-shell` window) over this device's own replica, in a UIKit window. The Xcode host in
//! the RoostrIOS repository links this static library and calls `roostr_main`.
//!
//! - [`app`]: the application and scene delegates, the window, and the state it runs.
//! - [`host`]: Rune's `LayerHost` drawing into the Metal view, ticked by a display link only
//!   while there is something to do; the system pasteboard.
//! - [`views`]: the root view controller and the Metal view (touches, hardware keys, trackpad
//!   scrolls, the edit menu).
//! - [`keyboard`], [`text_view`], [`text_bridge`]: the on-screen keyboard. Rune has no iOS text
//!   input of its own: a hidden `UITextInput` view takes the keyboard while a Rune field has
//!   the caret and turns what UIKit types, composes and corrects into Rune's keys and input
//!   method events.
//! - [`local`], [`network`], [`reminders`], [`reduce_motion`]: the Keychain and `UserDefaults`
//!   values, reconnecting on network changes, reminders as local notifications, Reduce Motion.
//!
//! On other targets the crate holds only what its tests check on the host: the keyboard
//! bridge's model and the deep links.

#[cfg(any(test, target_os = "ios"))]
pub mod links;
#[cfg(any(test, target_os = "ios"))]
pub mod text_bridge;

#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
mod host;
#[cfg(target_os = "ios")]
mod keyboard;
#[cfg(target_os = "ios")]
mod local;
#[cfg(target_os = "ios")]
mod network;
#[cfg(target_os = "ios")]
mod pasteboard;
#[cfg(target_os = "ios")]
mod reduce_motion;
#[cfg(target_os = "ios")]
mod reminders;
#[cfg(target_os = "ios")]
mod text_view;
#[cfg(target_os = "ios")]
mod views;
