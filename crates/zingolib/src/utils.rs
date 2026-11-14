//! General library utilities such as parsing and conversions.

use std::time::SystemTime;
pub mod conversion;
pub mod error;

/// The operating system's randomness, unwrapped: `rand` 0.10 made the system
/// RNG fallible, and the wallet keeps the previous behaviour of panicking if
/// the OS cannot supply entropy rather than signing with anything weaker.
#[must_use]
pub fn system_rng() -> rand::rand_core::UnwrapErr<rand::rngs::SysRng> {
    rand::rand_core::UnwrapErr(rand::rngs::SysRng)
}

/// Returns number of seconds since unix epoch.
pub(crate) fn now() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("should never fail when comparing with an instant so far in the past")
        .as_secs() as u32
}
