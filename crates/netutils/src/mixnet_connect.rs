//! Pure helpers of the mixnet proxy lifecycle (ADR 0011).
//!
//! [`NymProxy`](crate::NymProxy) is gated on the `nym` feature and its
//! collaborators (the mixnet client, the discovery API) cannot be constructed
//! in a unit test, so any logic left inline there is untestable in CI's
//! default build. This module holds small pure functions (input in, output
//! out, entropy injected) and is deliberately NOT feature-gated, so these
//! tests run in the default build without the nym-sdk stack. The connect
//! escalation logic itself lives in the shared planner,
//! [`crate::arm_race`].
#![forbid(unsafe_code)]

/// Fisher-Yates shuffle driven by a caller-supplied seed, so the permutation
/// is a pure function of `(items, seed)`. The pure core of Exit Node
/// shuffling: the caller supplies entropy (production hashes the clock,
/// tests pass a constant).
///
/// NOTE: the underlying generator is a plain LCG, NOT cryptographically
/// secure. Its purpose is load distribution across exit gateways, not
/// unpredictability. The mixnet's privacy comes from Sphinx routing, not
/// from which gateway is chosen.
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) fn seeded_shuffle<T>(items: &mut [T], mut seed: u64) {
    for i in (1..items.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (seed as usize) % (i + 1);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests;
