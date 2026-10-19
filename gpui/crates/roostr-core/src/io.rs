//! Where network work runs. The desktop clients block (ureq) and run on GPUI's
//! background threads; the browser has one thread, and its fetch futures are
//! not `Send`, so they run on the foreground executor. Code that awaits a
//! [`crate::Backend`] or [`crate::harness::Harness`] future spawns it with
//! [`spawn`] (or [`spawn_async`]) and never awaits it on the UI thread
//! directly: on the desktop the future blocks while it is polled.

use futures::Stream;
#[cfg(not(target_family = "wasm"))]
use gpui_kit::AppContext as _;
use gpui_kit::{App, AsyncApp, Task};

/// Whether this build runs in a browser tab.
pub const WEB: bool = cfg!(target_family = "wasm");

/// Shown where the desktop opens a file dialog: GPUI's browser platform has
/// no file picker.
pub const NO_FILE_PICKER: &str = "Choosing files from this computer isn't available in the browser yet - use the Roostr desktop app.";

#[cfg(not(target_family = "wasm"))]
mod bounds {
    use std::pin::Pin;

    use futures::Stream;

    /// `Send` on the desktop, nothing in the browser.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    /// `Sync` on the desktop, nothing in the browser.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}

    pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
    pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
}

#[cfg(target_family = "wasm")]
mod bounds {
    use std::pin::Pin;

    use futures::Stream;

    /// `Send` on the desktop, nothing in the browser.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    /// `Sync` on the desktop, nothing in the browser.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}

    pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;
    pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T>>>;
}

pub use bounds::{BoxFuture, BoxStream, MaybeSend, MaybeSync};

/// Box a future as a [`BoxFuture`].
pub fn boxed<F>(future: F) -> BoxFuture<F::Output>
where
    F: Future + MaybeSend + 'static,
{
    Box::pin(future)
}

/// Box a stream as a [`BoxStream`].
pub fn boxed_stream<S>(stream: S) -> BoxStream<S::Item>
where
    S: Stream + MaybeSend + 'static,
{
    Box::pin(stream)
}

/// Run network work off the UI: a background thread on the desktop, the
/// browser's event loop on the web.
pub fn spawn<T>(cx: &App, future: impl Future<Output = T> + MaybeSend + 'static) -> Task<T>
where
    T: MaybeSend + 'static,
{
    #[cfg(not(target_family = "wasm"))]
    {
        cx.background_spawn(future)
    }
    #[cfg(target_family = "wasm")]
    {
        cx.foreground_executor().spawn(future)
    }
}

/// [`spawn`] from an async context.
pub fn spawn_async<T>(
    cx: &AsyncApp,
    future: impl Future<Output = T> + MaybeSend + 'static,
) -> Task<T>
where
    T: MaybeSend + 'static,
{
    #[cfg(not(target_family = "wasm"))]
    {
        cx.background_executor().spawn(future)
    }
    #[cfg(target_family = "wasm")]
    {
        cx.foreground_executor().spawn(future)
    }
}
