//! Clock and timers that work under any executor: `async-io` timers on
//! native, `setTimeout` in the browser.

use std::pin::pin;
use std::time::Duration;

use futures::future::{Either, select};

/// Seconds since the Unix epoch (`Math.floor(Date.now() / 1000)`).
pub fn now_s() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Milliseconds since the Unix epoch (`Date.now()`).
pub(crate) fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Resolves after `duration`.
pub async fn sleep(duration: Duration) {
    #[cfg(not(target_family = "wasm"))]
    {
        async_io::Timer::after(duration).await;
    }
    #[cfg(target_family = "wasm")]
    {
        // setTimeout takes a 32-bit millisecond delay.
        let ms = duration.as_millis().min(i32::MAX as u128) as u32;
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
}

/// `future`'s output, or `None` when `duration` passes first (the future is
/// dropped).
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Option<F::Output> {
    match select(pin!(future), pin!(sleep(duration))).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(((), _)) => None,
    }
}
