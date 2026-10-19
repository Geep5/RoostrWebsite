//! The platform CSPRNG (`crypto.getRandomValues` in the browser).

/// `N` random bytes. The system source failing is unrecoverable for a
/// signer: there is no safe fallback.
pub(crate) fn bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom_02::getrandom(&mut out).expect("the system random source is unavailable");
    out
}

/// A uniform integer in `0..=max`.
pub(crate) fn up_to(max: u64) -> u64 {
    if max == u64::MAX {
        return u64::from_le_bytes(bytes());
    }
    let span = max + 1;
    // Rejection sampling: no modulo bias.
    let zone = u64::MAX - (u64::MAX % span) - 1;
    loop {
        let value = u64::from_le_bytes(bytes());
        if value <= zone {
            return value % span;
        }
    }
}
