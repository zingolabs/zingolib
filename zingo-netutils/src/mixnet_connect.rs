//! Pure helpers of the mixnet proxy lifecycle (ADR 0011).
//!
//! [`NymProxy`](crate::NymProxy) is gated on the `nym` feature and its
//! collaborators (the mixnet client, the discovery API) cannot be constructed
//! in a unit test, so any logic left inline there is untestable in CI's
//! default build. This module holds small pure functions — input in, output
//! out, entropy injected — and is deliberately NOT feature-gated, so these
//! tests run in the default build without the nym-sdk stack. The connect
//! escalation logic itself lives in the shared planner,
//! [`crate::arm_race`].
#![forbid(unsafe_code)]

/// Strip the `socks5h://` scheme from a SOCKS5 URL, passing a bare
/// `host:port` through unchanged. The pure core of
/// [`NymProxy::socks5_addr`](crate::NymProxy::socks5_addr).
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) fn strip_socks5_scheme(url: &str) -> &str {
    url.strip_prefix("socks5h://").unwrap_or(url)
}

/// Fisher-Yates shuffle driven by a caller-supplied seed, so the permutation
/// is a pure function of `(items, seed)`. The pure core of provider
/// shuffling: the caller supplies entropy (production hashes the clock;
/// tests pass a constant).
///
/// NOTE: the underlying generator is a plain LCG — NOT cryptographically
/// secure. Its purpose is load distribution across exit gateways, not
/// unpredictability; the mixnet's privacy comes from Sphinx routing, not
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
mod tests {
    use super::*;

    #[test]
    fn strips_the_socks5h_scheme() {
        assert_eq!(
            strip_socks5_scheme("socks5h://127.0.0.1:1080"),
            "127.0.0.1:1080"
        );
    }

    #[test]
    fn passes_a_bare_address_through() {
        assert_eq!(strip_socks5_scheme("127.0.0.1:1080"), "127.0.0.1:1080");
    }

    #[test]
    fn a_seed_fixes_the_permutation() {
        let mut first = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mut second = first.clone();
        seeded_shuffle(&mut first, 42);
        seeded_shuffle(&mut second, 42);
        assert_eq!(first, second, "the same seed yields the same order");
    }

    #[test]
    fn shuffling_permutes_without_loss() {
        let mut items = vec![1, 2, 3, 4, 5, 6, 7, 8];
        seeded_shuffle(&mut items, 7);
        let mut sorted = items.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn empty_and_singleton_slices_are_untouched() {
        let mut empty: Vec<u8> = vec![];
        seeded_shuffle(&mut empty, 1);
        assert!(empty.is_empty());

        let mut one = vec![9];
        seeded_shuffle(&mut one, 1);
        assert_eq!(one, vec![9]);
    }
}
