//! Opt-in sync tracing for debugging a replica that misses what other
//! devices see: each relay subscription, every event received and what the
//! engine made of it, what was imported or refused, each object replayed and
//! the chat a discussion then shows. Off unless switched on; in the browser:
//!
//! ```js
//! localStorage.setItem("roostr.syncTrace", "1"); location.reload()
//! ```
//!
//! and `localStorage.removeItem("roostr.syncTrace")` to stop. Lines start
//! with `[trace]` and go to the console as plain logs.

use std::sync::atomic::{AtomicBool, Ordering};

/// The `localStorage` key that switches tracing on (value `"1"`).
pub const STORAGE_KEY: &str = "roostr.syncTrace";

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Write one trace line, whatever the log level (use [`sync_trace!`]).
pub fn emit(args: std::fmt::Arguments) {
    let line = format!("[trace] {args}");
    #[cfg(target_family = "wasm")]
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&line));
    #[cfg(not(target_family = "wasm"))]
    eprintln!("{line}");
}

/// The first `n` characters of `s`, for ids and previews in trace lines.
pub fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `format!`-style trace line, built only while tracing is on.
#[macro_export]
macro_rules! sync_trace {
    ($($arg:tt)*) => {
        if $crate::sync_trace::enabled() {
            $crate::sync_trace::emit(format_args!($($arg)*));
        }
    };
}
