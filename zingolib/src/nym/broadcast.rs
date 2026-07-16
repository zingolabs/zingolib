//! Witness-rotation broadcast over the Nym mixnet.
//!
//! Each Transmission picks a single Broadcast Indexer at random from the
//! curated list and submits over the mixnet; on an unreachable pick it fails
//! over to a fresh random indexer, trying at most a bounded number of
//! distinct indexers. A substantive rejection is terminal, because an invalid
//! transaction rejects everywhere. The same transaction is never fired to
//! more than one indexer at once: this is failover, not redundancy.
//!
//! The transport is abstracted behind [`Transmitter`] and the choice of
//! indexer behind an injected [`Rng`], so the whole rotation-and-failover
//! logic is exercised in CI against a mock transmitter and a seeded
//! generator. The live nym-sdk SOCKS5 transport slots in behind the same
//! trait in a later increment. See `docs/adr/0011-nym-mixnet-transmission.md`.
#![forbid(unsafe_code)]

use std::future::Future;

use http::Uri;
use rand::Rng;
use rand::seq::SliceRandom;

/// Why an indexer did not accept a submitted transaction.
#[derive(Clone, Debug)]
pub enum SubmitError {
    /// The indexer could not be reached; a candidate for failover to a
    /// different Broadcast Indexer.
    Unreachable(String),
    /// The indexer rejected the transaction on its merits. An invalid
    /// transaction rejects everywhere, so failover is pointless.
    Rejected(String),
}

/// Submits a raw transaction to a single indexer. The live nym-sdk mixnet
/// transport implements this in production; a mock implements it in tests.
pub trait Transmitter {
    /// Submits `raw_tx` to `indexer`, returning the server-reported txid on
    /// acceptance. A duplicate already in the mempool or chain counts as
    /// acceptance.
    fn submit(
        &self,
        indexer: &Uri,
        raw_tx: &[u8],
    ) -> impl Future<Output = Result<String, SubmitError>> + Send;
}

/// A successful witness-rotation broadcast.
#[derive(Clone, Debug)]
pub struct BroadcastOk {
    /// The Broadcast Indexer that accepted the transaction.
    pub indexer: Uri,
    /// The server-reported txid.
    pub server_txid: String,
}

/// Why a witness-rotation broadcast failed.
#[derive(Clone, Debug)]
pub enum BroadcastError {
    /// An indexer rejected the transaction on its merits; not retried.
    Rejected(String),
    /// Every attempted indexer was unreachable within the attempt bound.
    AllUnreachable {
        /// The number of distinct indexers tried.
        attempts: usize,
    },
    /// The Broadcast Indexer list was empty.
    NoIndexers,
}

/// Broadcasts `raw_tx` to one Broadcast Indexer chosen at random from
/// `indexers` (witness rotation), failing over to a fresh random indexer on
/// an unreachable pick, trying at most `max_attempts` distinct indexers.
///
/// Exactly one submission is ever in flight. A substantive
/// [`SubmitError::Rejected`] returns immediately, since an invalid
/// transaction rejects everywhere.
pub async fn broadcast<T, R>(
    transmitter: &T,
    indexers: &[Uri],
    rng: &mut R,
    raw_tx: &[u8],
    max_attempts: usize,
) -> Result<BroadcastOk, BroadcastError>
where
    T: Transmitter + Sync,
    R: Rng + ?Sized,
{
    if indexers.is_empty() {
        return Err(BroadcastError::NoIndexers);
    }

    // A random permutation yields both the initial random pick and a
    // repetition-free random failover order in one shot.
    let mut order: Vec<usize> = (0..indexers.len()).collect();
    order.shuffle(rng);

    let bound = max_attempts.min(order.len());
    let mut attempts = 0;
    for &index in order.iter().take(bound) {
        let indexer = &indexers[index];
        attempts += 1;
        match transmitter.submit(indexer, raw_tx).await {
            Ok(server_txid) => {
                return Ok(BroadcastOk {
                    indexer: indexer.clone(),
                    server_txid,
                });
            }
            Err(SubmitError::Unreachable(_)) => continue,
            Err(SubmitError::Rejected(message)) => {
                return Err(BroadcastError::Rejected(message));
            }
        }
    }

    Err(BroadcastError::AllUnreachable { attempts })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn uris(hosts: &[&str]) -> Vec<Uri> {
        hosts.iter().map(|h| h.parse().unwrap()).collect()
    }

    /// A transmitter returning a scripted result per indexer host, recording
    /// the order of the hosts it was asked to submit to.
    struct MockTransmitter {
        results: HashMap<String, Result<String, SubmitError>>,
        calls: Mutex<Vec<String>>,
    }

    impl MockTransmitter {
        fn new(scripts: &[(&str, Result<String, SubmitError>)]) -> Self {
            let results = scripts
                .iter()
                .map(|(host, result)| ((*host).to_string(), result.clone()))
                .collect();
            MockTransmitter {
                results,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transmitter for MockTransmitter {
        fn submit(
            &self,
            indexer: &Uri,
            _raw_tx: &[u8],
        ) -> impl Future<Output = Result<String, SubmitError>> + Send {
            let host = indexer.host().unwrap().to_string();
            self.calls.lock().unwrap().push(host.clone());
            let result = self
                .results
                .get(&host)
                .cloned()
                .unwrap_or(Err(SubmitError::Unreachable("no script".into())));
            async move { result }
        }
    }

    fn rng() -> StdRng {
        StdRng::seed_from_u64(0xC0FFEE)
    }

    #[tokio::test]
    async fn single_indexer_accepts() {
        let mock = MockTransmitter::new(&[("a.example", Ok("txid-a".into()))]);
        let indexers = uris(&["https://a.example:443"]);

        let ok = broadcast(&mock, &indexers, &mut rng(), b"tx", 4)
            .await
            .expect("the sole indexer accepts");

        assert_eq!(ok.server_txid, "txid-a");
        assert_eq!(ok.indexer.host(), Some("a.example"));
        assert_eq!(mock.calls(), vec!["a.example"]);
    }

    #[tokio::test]
    async fn substantive_rejection_is_terminal_and_not_retried() {
        let mock = MockTransmitter::new(&[(
            "a.example",
            Err(SubmitError::Rejected("invalid transaction".into())),
        )]);
        let indexers = uris(&["https://a.example:443"]);

        let err = broadcast(&mock, &indexers, &mut rng(), b"tx", 4)
            .await
            .expect_err("a substantive rejection fails the broadcast");

        assert!(matches!(err, BroadcastError::Rejected(m) if m == "invalid transaction"));
        // A single indexer, called once: rejection did not trigger failover.
        assert_eq!(mock.calls(), vec!["a.example"]);
    }

    #[tokio::test]
    async fn fails_over_past_unreachable_to_an_acceptor() {
        // Exactly one indexer accepts; the other two are unreachable. Whatever
        // the random order, the broadcast must skip the unreachable ones and
        // land on the acceptor.
        let mock = MockTransmitter::new(&[
            ("down1.example", Err(SubmitError::Unreachable("no route".into()))),
            ("up.example", Ok("txid-up".into())),
            ("down2.example", Err(SubmitError::Unreachable("no route".into()))),
        ]);
        let indexers = uris(&[
            "https://down1.example:443",
            "https://up.example:443",
            "https://down2.example:443",
        ]);

        let ok = broadcast(&mock, &indexers, &mut rng(), b"tx", 4)
            .await
            .expect("failover reaches the one reachable indexer");

        assert_eq!(ok.server_txid, "txid-up");
        assert_eq!(ok.indexer.host(), Some("up.example"));
        assert!(mock.calls().contains(&"up.example".to_string()));
    }

    #[tokio::test]
    async fn all_unreachable_reports_every_distinct_attempt() {
        let mock = MockTransmitter::new(&[
            ("a.example", Err(SubmitError::Unreachable("x".into()))),
            ("b.example", Err(SubmitError::Unreachable("x".into()))),
            ("c.example", Err(SubmitError::Unreachable("x".into()))),
        ]);
        let indexers = uris(&[
            "https://a.example:443",
            "https://b.example:443",
            "https://c.example:443",
        ]);

        let err = broadcast(&mock, &indexers, &mut rng(), b"tx", 10)
            .await
            .expect_err("no indexer is reachable");

        assert!(matches!(err, BroadcastError::AllUnreachable { attempts: 3 }));
        // Every indexer tried exactly once — failover never repeats a pick.
        let mut calls = mock.calls();
        calls.sort();
        assert_eq!(calls, vec!["a.example", "b.example", "c.example"]);
    }

    #[tokio::test]
    async fn stops_at_max_attempts_without_walking_the_list() {
        let mock = MockTransmitter::new(&[
            ("a.example", Err(SubmitError::Unreachable("x".into()))),
            ("b.example", Err(SubmitError::Unreachable("x".into()))),
            ("c.example", Err(SubmitError::Unreachable("x".into()))),
            ("d.example", Err(SubmitError::Unreachable("x".into()))),
            ("e.example", Err(SubmitError::Unreachable("x".into()))),
        ]);
        let indexers = uris(&[
            "https://a.example:443",
            "https://b.example:443",
            "https://c.example:443",
            "https://d.example:443",
            "https://e.example:443",
        ]);

        let err = broadcast(&mock, &indexers, &mut rng(), b"tx", 2)
            .await
            .expect_err("bounded attempts exhausted");

        assert!(matches!(err, BroadcastError::AllUnreachable { attempts: 2 }));
        assert_eq!(mock.calls().len(), 2, "the attempt bound is respected");
    }

    #[tokio::test]
    async fn empty_list_is_no_indexers() {
        let mock = MockTransmitter::new(&[]);
        let err = broadcast(&mock, &[], &mut rng(), b"tx", 4)
            .await
            .expect_err("an empty broadcast list cannot send");
        assert!(matches!(err, BroadcastError::NoIndexers));
        assert!(mock.calls().is_empty());
    }

    #[tokio::test]
    async fn injected_rng_makes_the_pick_order_reproducible() {
        // The same seed must produce the same failover order, proving the
        // generator is injected rather than drawn from a global source.
        let script: Vec<(&str, Result<String, SubmitError>)> = vec![
            ("a.example", Err(SubmitError::Unreachable("x".into()))),
            ("b.example", Err(SubmitError::Unreachable("x".into()))),
            ("c.example", Err(SubmitError::Unreachable("x".into()))),
            ("d.example", Err(SubmitError::Unreachable("x".into()))),
        ];
        let indexers = uris(&[
            "https://a.example:443",
            "https://b.example:443",
            "https://c.example:443",
            "https://d.example:443",
        ]);

        let first = MockTransmitter::new(&script);
        let _ = broadcast(&first, &indexers, &mut StdRng::seed_from_u64(42), b"tx", 10).await;

        let second = MockTransmitter::new(&script);
        let _ = broadcast(&second, &indexers, &mut StdRng::seed_from_u64(42), b"tx", 10).await;

        assert_eq!(
            first.calls(),
            second.calls(),
            "one seed, one deterministic pick order"
        );
    }
}
