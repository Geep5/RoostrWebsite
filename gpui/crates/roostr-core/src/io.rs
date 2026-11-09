//! Where network work runs. The desktop clients block (ureq) and run on
//! roostr-rt's worker threads; the browser has one thread, and its fetch
//! futures are not `Send`, so they run on the UI thread's executor. Code that
//! awaits a [`crate::Backend`] or [`crate::harness::Harness`] future runs it
//! through [`roostr_rt::Executor::background`] and never awaits it on the UI
//! thread directly: on the desktop the future blocks while it is polled.

/// Whether this build runs in a browser tab.
pub const WEB: bool = cfg!(target_family = "wasm");

/// Shown where the desktop opens a file dialog: the browser build has no
/// file picker.
pub const NO_FILE_PICKER: &str = "Choosing files from this computer isn't available in the browser yet - use the Roostr desktop app.";
