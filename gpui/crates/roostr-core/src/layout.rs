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

use std::rc::Rc;

use crate::State;

/// `matchMedia("(max-width: 720px)")`: the mobile shell's breakpoint, in
/// logical pixels.
pub const MOBILE_MAX_WIDTH: f32 = 720.;

/// A window `width` logical pixels wide is narrow enough for the mobile
/// shell (`isMobile` in `routes/app/+layout.svelte`, `isMobileVp` on the
/// object page, every `@media (max-width: 720px)` rule).
pub fn is_mobile(width: f32) -> bool {
    width <= MOBILE_MAX_WIDTH
}

/// `@media (hover: none)`: a touch screen is the pointer, so hover-revealed
/// controls show at rest and long presses stand in for right clicks. A
/// property of the platform built for (iOS, Android), not of the window.
pub const fn is_touch() -> bool {
    cfg!(any(target_os = "ios", target_os = "android"))
}

/// The host's "sync now" (iOS `Backend.resume`: reconnect every relay and
/// catch up), in [`State::sync_now`]. The mobile header's sync dot is a
/// button only where the host set one, as the Svelte dot was enabled only on
/// the iOS backend.
#[derive(Clone)]
pub struct SyncNow(pub Rc<dyn Fn(&mut State)>);
