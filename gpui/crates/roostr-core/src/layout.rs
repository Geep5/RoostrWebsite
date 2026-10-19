//! Viewport-driven layout: the Svelte app's mobile shell and its
//! `@media (max-width: 720px)` rules, and `@media (hover: none)` touch rules.
//! Every front end shares them: a phone browser tab, a narrow desktop
//! window, and the iOS app (where iPhone portrait is narrow, while iPhone
//! landscape and iPad lay out like the desktop, as the web app did in the
//! iOS web view).
//!
//! `env(safe-area-inset-*)` is zero inside the app on every host: the iOS
//! app keeps its window content inside the safe area and above the
//! on-screen keyboard, where the iOS web view sat.

use gpui_kit::{Pixels, Window, px};

/// `matchMedia("(max-width: 720px)")`: the mobile shell's breakpoint.
pub const MOBILE_MAX_WIDTH: Pixels = px(720.);

/// The window is narrow enough for the mobile shell (`isMobile` in
/// `routes/app/+layout.svelte`, `isMobileVp` on the object page, every
/// `@media (max-width: 720px)` rule).
pub fn is_mobile(window: &Window) -> bool {
    window.viewport_size().width <= MOBILE_MAX_WIDTH
}

/// `@media (hover: none)`: a touch screen is the pointer, so hover-revealed
/// controls show at rest and long presses stand in for right clicks.
pub fn is_touch() -> bool {
    gpui_kit::is_mobile()
}

/// The host's "sync now" (iOS `Backend.resume`: reconnect every relay and
/// catch up). The mobile header's sync dot is a button only where the host
/// set one, as the Svelte dot was enabled only on the iOS backend.
#[derive(Clone)]
pub struct SyncNow(pub std::rc::Rc<dyn Fn(&mut gpui_kit::App)>);

impl gpui_kit::Global for SyncNow {}

/// The host's [`SyncNow`], when it has one.
pub fn sync_now(cx: &gpui_kit::App) -> Option<SyncNow> {
    cx.try_global::<SyncNow>().cloned()
}

/// CSS `100vw`/`100dvh`: the size of the area the app is laid out in. On
/// iOS that is the window inside the safe area and above the on-screen
/// keyboard (the web view's frame); elsewhere it is the whole window.
pub fn viewport(window: &Window) -> gpui_kit::Size<Pixels> {
    window.fully_visible_bounds().size
}
