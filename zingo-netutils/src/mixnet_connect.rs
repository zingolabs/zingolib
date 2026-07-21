//! The pure core of the mixnet proxy lifecycle (ADR 0011).
//!
//! [`NymProxy`](crate::NymProxy) is gated on the `nym` feature and its
//! collaborators (the mixnet client, the discovery API) cannot be constructed
//! in a unit test, so any logic left inline there is untestable in CI's
//! default build. This module holds that logic as pure functions — input in,
//! output out, with the effectful collaborators (the connection attempt, the
//! retry cadence, the entropy source) injected as arguments — and is
//! deliberately NOT feature-gated, so these tests run in the default build
//! without the nym-sdk stack.
#![forbid(unsafe_code)]

use std::future::Future;
use std::time::Duration;

/// Strip the `socks5h://` scheme from a SOCKS5 URL, passing a bare
/// `host:port` through unchanged. The pure core of
/// [`NymProxy::socks5_addr`](crate::NymProxy::socks5_addr).
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) fn strip_socks5_scheme(url: &str) -> &str {
    url.strip_prefix("socks5h://").unwrap_or(url)
}

/// Try `connect` against each of the first `max_providers_per_round`
/// providers, for up to `rounds` rounds with `sleep(round_interval)` between
/// rounds, returning the first success. On exhaustion returns the last
/// connection error, or `None` when `providers` is empty and nothing was
/// attempted; the caller maps that to its no-provider error.
///
/// This is the single definition of the retry engine that
/// [`NymProxy::start`](crate::NymProxy::start) and
/// [`NymProxy::reconnect`](crate::NymProxy::reconnect) previously each
/// spelled out inline. The connection attempt and the cadence are both
/// injected, so the round and cap logic is exercised without a live mixnet
/// or real time.
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) async fn connect_with_retries<P, T, E, C, F, S, SF>(
    providers: &[P],
    max_providers_per_round: usize,
    rounds: usize,
    round_interval: Duration,
    connect: C,
    mut sleep: S,
) -> Result<T, Option<E>>
where
    P: Clone,
    C: Fn(P) -> F,
    F: Future<Output = Result<T, E>>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let attempts = providers.len().min(max_providers_per_round);
    if attempts == 0 {
        return Err(None);
    }

    let mut last_err = None;
    for _round in 0..rounds {
        for provider in providers.iter().take(attempts) {
            match connect(provider.clone()).await {
                Ok(connected) => return Ok(connected),
                Err(e) => last_err = Some(e),
            }
        }
        sleep(round_interval).await;
    }
    Err(last_err)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// A no-op sleep so the retry cadence costs no real time in tests.
    fn no_sleep(_: Duration) -> std::future::Ready<()> {
        std::future::ready(())
    }

    const INTERVAL: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn first_provider_success_connects_once_and_never_sleeps() {
        let connects = AtomicUsize::new(0);
        let sleeps = AtomicUsize::new(0);
        let out = connect_with_retries(
            &["a", "b"],
            10,
            10,
            INTERVAL,
            |p| {
                connects.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Ok::<_, String>(p))
            },
            |d| {
                sleeps.fetch_add(1, Ordering::Relaxed);
                no_sleep(d)
            },
        )
        .await
        .expect("first provider connects");
        assert_eq!(out, "a");
        assert_eq!(connects.load(Ordering::Relaxed), 1);
        assert_eq!(sleeps.load(Ordering::Relaxed), 0, "no round completed");
    }

    #[tokio::test]
    async fn a_later_provider_rescues_the_round() {
        let out = connect_with_retries(
            &["down", "up"],
            10,
            10,
            INTERVAL,
            |p| {
                std::future::ready(if p == "up" {
                    Ok(p)
                } else {
                    Err(format!("{p} unreachable"))
                })
            },
            no_sleep,
        )
        .await
        .expect("second provider connects");
        assert_eq!(out, "up");
    }

    #[tokio::test]
    async fn exhaustion_reports_the_last_error_after_all_rounds() {
        let connects = AtomicUsize::new(0);
        let sleeps = AtomicUsize::new(0);
        let err = connect_with_retries(
            &["a", "b", "c"],
            10,
            4,
            INTERVAL,
            |p: &str| {
                connects.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Err::<(), _>(format!("{p} down")))
            },
            |d| {
                sleeps.fetch_add(1, Ordering::Relaxed);
                no_sleep(d)
            },
        )
        .await
        .expect_err("every attempt fails");
        assert_eq!(err, Some("c down".to_string()), "the LAST error surfaces");
        assert_eq!(
            connects.load(Ordering::Relaxed),
            3 * 4,
            "every provider, every round"
        );
        assert_eq!(sleeps.load(Ordering::Relaxed), 4, "one sleep per round");
    }

    #[tokio::test]
    async fn the_per_round_provider_cap_holds() {
        let connects = AtomicUsize::new(0);
        let _ = connect_with_retries(
            &["a", "b", "c", "d", "e"],
            2,
            3,
            INTERVAL,
            |_p: &str| {
                connects.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Err::<(), _>("down".to_string()))
            },
            no_sleep,
        )
        .await;
        assert_eq!(
            connects.load(Ordering::Relaxed),
            2 * 3,
            "only the first two providers are tried each round"
        );
    }

    #[tokio::test]
    async fn no_providers_is_a_distinct_no_attempt_outcome() {
        let providers: &[&str] = &[];
        let err = connect_with_retries(
            providers,
            10,
            10,
            INTERVAL,
            |_p| std::future::ready(Err::<(), _>("unreachable".to_string())),
            no_sleep,
        )
        .await
        .expect_err("nothing to try");
        assert_eq!(err, None, "no attempt ran, so there is no last error");
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
