//! Native iOS platform support for GPUI.
//!
//! This crate connects GPUI to UIKit, CoreText, and Metal through the shared
//! Apple renderer. It owns iOS application lifecycle, windowing, native
//! text input, safe-area and keyboard insets, and raw touch delivery.

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(target_os = "ios")]
pub use ios::{IosPlatform, current_platform};

#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod pasted_image;

/// roostr: whether the on-screen keyboard's delete is the Backspace key
/// rather than a text edit: a caret at the start of the editable text,
/// outside a composition. Outside `ios` so the host's tests run it.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn backspace_at_start(
    selection: &std::ops::Range<usize>,
    editable_start: usize,
    composing: bool,
) -> bool {
    !composing && selection.is_empty() && selection.start <= editable_start
}

/// The foreground color used by the iOS status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusBarContentStyle {
    /// Light status-bar content for dark backgrounds.
    Light,
    /// Dark status-bar content for light backgrounds.
    #[default]
    Dark,
}

#[cfg(test)]
mod tests {
    use super::backspace_at_start;

    #[test]
    fn delete_at_start_is_the_backspace_key() {
        // Caret at the start (an empty field, or text after it): the key.
        assert!(backspace_at_start(&(0..0), 0, false));
        assert!(backspace_at_start(&(4..4), 4, false));
        // In the text, over a selection, or composing: a text edit.
        assert!(!backspace_at_start(&(3..3), 0, false));
        assert!(!backspace_at_start(&(0..3), 0, false));
        assert!(!backspace_at_start(&(0..0), 0, true));
    }
}
