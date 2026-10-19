//! The linked static library (`build.rs`): `glon.h` of the xcframework.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::{ABI_VERSION, EngineError};

unsafe extern "C" {
    fn core_abi_version() -> u32;
    fn core_init();
    fn core_reserve(length: u32) -> *mut c_void;
    fn core_reserve_blob(length: u32) -> *mut c_void;
    fn core_execute() -> u32;
    fn core_response_pointer() -> *const c_void;
    fn core_response_length() -> u32;
    fn core_reset(reset_all: u32);
}

/// `GLON_SOURCE_FINGERPRINT` from `build.rs`.
const SOURCE_FINGERPRINT: &str = env!("GLON_SOURCE_FINGERPRINT");

/// Serialises every call: the engine's buffers and caches are process-global.
static LOCK: Mutex<()> = Mutex::new(());
/// The linked ABI version, read once right after `core_init`.
static LINKED_ABI: LazyLock<u32> = LazyLock::new(|| {
    let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: runs exactly once, before any other export, under the lock.
    unsafe {
        core_init();
        core_abi_version()
    }
});
/// Bumped when the engine dropped its persistent state.
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub(crate) struct Host;

impl Host {
    pub(crate) fn get() -> Result<Host, EngineError> {
        let abi = *LINKED_ABI;
        if abi != ABI_VERSION {
            return Err(EngineError::AbiMismatch(abi));
        }
        Ok(Host)
    }

    pub(crate) fn source_fingerprint(&self) -> &str {
        SOURCE_FINGERPRINT
    }

    pub(crate) fn generation(&self) -> u64 {
        GENERATION.load(Ordering::Acquire)
    }

    pub(crate) fn reset_all(&self) {
        let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: the engine is initialised (`Host::get`) and no call is in flight.
        unsafe { core_reset(1) };
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }

    /// One single-flight request. `decode` reads the response in place,
    /// before the reset invalidates it. A runtime fault resets the whole
    /// engine and bumps the generation, as `core.ts` does after a trap.
    pub(crate) fn execute<R>(
        &self,
        request: &[u8],
        blob: Option<&[u8]>,
        decode: impl FnOnce(&[u8]) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: lengths are bounded by REQUEST_LIMIT / BLOB_LIMIT (checked by
        // the caller, so they fit u32); the engine owns the reserved buffers
        // until `core_reset`, and the response slice is dropped before it.
        let result = unsafe {
            let pointer = core_reserve(request.len() as u32);
            if pointer.is_null() {
                Err(EngineError::Runtime(
                    "engine refused the request reservation".into(),
                ))
            } else {
                std::ptr::copy_nonoverlapping(
                    request.as_ptr(),
                    pointer.cast::<u8>(),
                    request.len(),
                );
                reserve_blob(blob).and_then(|()| {
                    if core_execute() != 0 {
                        return Err(EngineError::Runtime(
                            "engine rejected the request state".into(),
                        ));
                    }
                    let output = core_response_pointer().cast::<u8>();
                    let length = core_response_length() as usize;
                    if output.is_null() || length == 0 {
                        return Err(EngineError::Runtime(
                            "invalid engine response bounds".into(),
                        ));
                    }
                    decode(std::slice::from_raw_parts(output, length))
                })
            }
        };
        let fault = matches!(result, Err(EngineError::Runtime(_)));
        // SAFETY: as above; the response slice is no longer borrowed.
        unsafe { core_reset(u32::from(fault)) };
        if fault {
            GENERATION.fetch_add(1, Ordering::AcqRel);
        }
        result
    }
}

/// # Safety
/// Called between `core_reserve` and `core_execute` under the engine lock.
unsafe fn reserve_blob(blob: Option<&[u8]>) -> Result<(), EngineError> {
    let Some(bytes) = blob else { return Ok(()) };
    // SAFETY: see the caller; the reservation holds `bytes.len()` bytes.
    unsafe {
        let pointer = core_reserve_blob(bytes.len() as u32);
        if pointer.is_null() {
            return Err(EngineError::BlobTooLarge(bytes.len()));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
    }
    Ok(())
}
