//! Detached background work without a runtime of our own: the sync loops,
//! deferred shared-space refreshes, owner signals.
//!
//! Natively each task gets a thread driven by `async_io::block_on` (the
//! relay sockets are `async-io`'s, and engine and store futures block while
//! polled, so they must not share GPUI's executors' threads for long). In the
//! browser everything runs on the page's event loop.

use crate::MaybeSend;

/// Run `future` to completion in the background; nothing awaits it.
pub fn spawn(future: impl Future<Output = ()> + MaybeSend + 'static) {
    #[cfg(not(target_family = "wasm"))]
    {
        let spawned = std::thread::Builder::new()
            .name("roostr-replica".into())
            .spawn(move || async_io::block_on(future));
        if let Err(error) = spawned {
            log::error!("[replica] cannot start a background task: {error}");
        }
    }
    #[cfg(target_family = "wasm")]
    {
        wasm_bindgen_futures::spawn_local(future);
    }
}
