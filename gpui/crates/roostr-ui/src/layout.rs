//! Viewport-driven layout: the Svelte app's mobile shell breakpoint
//! (`@media (max-width: 720px)`). A view learns its width from `El::on_layout` on its root and
//! keeps it in state.

/// `matchMedia("(max-width: 720px)")`: the mobile shell's breakpoint.
pub const MOBILE_MAX_WIDTH: f32 = 720.0;

/// A window `width` points wide is narrow enough for the mobile shell (`isMobile` in
/// `routes/app/+layout.svelte`, every `@media (max-width: 720px)` rule).
pub fn is_mobile(width: f32) -> bool {
    width <= MOBILE_MAX_WIDTH
}
