//! The resilient transmission policy: retry, duplicate-in-mempool, and
//! queued-probe handling, defined once and generic over the transmission
//! target so the clearnet indexer path and the Nym broadcast path share a
//! single implementation.
//!
//! [`resilient_transmit`] performs no wallet-state mutation — it interprets a
//! target's responses and returns the server-reported txid or a
//! [`TransmitFailed`]; the caller owns any wallet effects. The probe/retry
//! cadence is injected as a `sleep` hook so the policy is unit-tested without
//! real time.

use std::future::Future;
use std::time::Duration;

use zcash_primitives::transaction::TxId;

/// Transient-error retries before the delivery check.
pub(crate) const MAX_RETRIES: u8 = 3;

/// A "queued for download" duplicate rejection proves delivery but not
/// minability: zebra is still verifying the earlier submission (observed to
/// lag it by seconds under load). Each resubmission is a free probe of zebra's
/// own state; wait up to this many probes, on the retry loop's one-second
/// cadence, for the verdict to become storage-backed (issue #2450).
pub(crate) const MAX_QUEUED_PROBES: u8 = 30;

/// The interval between retries and queued-probes.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// A single transmission endpoint: submits a serialized transaction and can
/// ask the server whether it already knows a txid. Implemented for the
/// configured clearnet indexer and, later, for a Nym Broadcast Indexer reached
/// through the SOCKS5 proxy.
pub(crate) trait TransmitTarget {
    /// Submit `raw_tx` at `height`, returning the server-reported txid on
    /// acceptance, or the server/transport failure message. The message is
    /// classified by [`resilient_transmit`] (duplicate, queued, or transient).
    fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> impl Future<Output = Result<String, String>> + Send;

    /// Whether the server knows `txid` — a delivery check run after the
    /// retries are exhausted, since a lost response can mask a received
    /// transaction.
    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send;
}

/// The transmission exhausted the resilience policy: retries or queued-probes
/// ran out and the server does not know the transaction. Carries the last
/// server/transport message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransmitFailed(pub String);

/// How the resilience policy reads a submission failure message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RejectionClass {
    /// An earlier submission is minable (in the mempool) or already mined,
    /// so transmission is complete and the rejection counts as success.
    StorageBackedDuplicate,
    /// "Queued for download": the earlier submission was delivered but zebra
    /// has not yet verified it, so success is held until the verdict is
    /// storage-backed.
    QueuedProbe,
    /// Anything else: possibly transient, worth a bounded retry.
    Transient,
}

/// Classify a server/transport failure message for the resilience policy.
/// Substring matches because zainod surfaces the rejections untyped
/// (zingolabs/zaino#1392); when that lands, typed checks replace this
/// classifier and the policy loop is untouched.
pub(crate) fn classify_rejection(message: &str) -> RejectionClass {
    if message.contains("transaction already exists in mempool")
        || message.contains("transaction already in block chain")
    {
        return RejectionClass::StorageBackedDuplicate;
    }
    if message.contains("already queued for download") {
        return RejectionClass::QueuedProbe;
    }
    RejectionClass::Transient
}

/// Submit `raw_tx` to `target` under the shared resilience policy — the single
/// definition of the retry / duplicate-in-mempool / queued-probe behavior.
///
/// A duplicate already in the mempool or chain counts as success (an earlier
/// submission is minable or mined). "Queued for download" is re-probed up to
/// [`MAX_QUEUED_PROBES`] times until the verdict is storage-backed. Any other
/// error retries up to [`MAX_RETRIES`] times; on exhaustion a delivery check
/// ([`TransmitTarget::knows_transaction`]) confirms whether an earlier attempt
/// was in fact received. Returns the server-reported txid on success.
///
/// `sleep` supplies the wait between probes/retries; production passes
/// `tokio::time::sleep`, tests pass a no-op so the policy runs instantly.
pub(crate) async fn resilient_transmit<T, S, F>(
    target: &T,
    raw_tx: &[u8],
    height: u64,
    txid: &TxId,
    mut sleep: S,
) -> Result<String, TransmitFailed>
where
    T: TransmitTarget + Sync,
    S: FnMut(Duration) -> F,
    F: Future<Output = ()>,
{
    let mut retry_count: u8 = 0;
    let mut queued_probes: u8 = 0;

    loop {
        let message = match target.submit(raw_tx, height).await {
            Ok(server_txid) => return Ok(server_txid),
            Err(message) => message,
        };

        match classify_rejection(&message) {
            RejectionClass::StorageBackedDuplicate => return Ok(txid.to_string()),
            RejectionClass::QueuedProbe => {
                if queued_probes >= MAX_QUEUED_PROBES {
                    return Err(TransmitFailed(message));
                }
                queued_probes += 1;
                sleep(RETRY_INTERVAL).await;
            }
            RejectionClass::Transient => {
                if retry_count >= MAX_RETRIES {
                    // A transmission error does not prove the transaction
                    // failed to reach the network; an earlier attempt may have
                    // been accepted with its response lost (e.g. a timeout),
                    // causing rebroadcasts to be rejected as duplicates. Only
                    // fail if the server does not know it.
                    if target.knows_transaction(txid).await {
                        return Ok(txid.to_string());
                    }
                    return Err(TransmitFailed(message));
                }
                retry_count += 1;
                sleep(RETRY_INTERVAL).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A target that replays a scripted sequence of submit responses and a
    /// fixed delivery-check verdict, counting the calls it received. Atomics
    /// keep it `Sync` (the crate forbids unsafe) so its futures stay `Send`.
    struct ScriptedTarget {
        submits: Vec<Result<String, String>>,
        next: AtomicUsize,
        knows: bool,
        submit_calls: AtomicUsize,
        knows_calls: AtomicUsize,
    }

    impl ScriptedTarget {
        fn new(submits: Vec<Result<String, String>>, knows: bool) -> Self {
            ScriptedTarget {
                submits,
                next: AtomicUsize::new(0),
                knows,
                submit_calls: AtomicUsize::new(0),
                knows_calls: AtomicUsize::new(0),
            }
        }

        fn submit_calls(&self) -> usize {
            self.submit_calls.load(Ordering::Relaxed)
        }

        fn knows_calls(&self) -> usize {
            self.knows_calls.load(Ordering::Relaxed)
        }
    }

    impl TransmitTarget for ScriptedTarget {
        fn submit(
            &self,
            _raw_tx: &[u8],
            _height: u64,
        ) -> impl Future<Output = Result<String, String>> + Send {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            self.submit_calls.fetch_add(1, Ordering::Relaxed);
            let result = self
                .submits
                .get(i)
                .cloned()
                .unwrap_or_else(|| Err("script exhausted".to_string()));
            async move { result }
        }

        fn knows_transaction(&self, _txid: &TxId) -> impl Future<Output = bool> + Send {
            self.knows_calls.fetch_add(1, Ordering::Relaxed);
            let knows = self.knows;
            async move { knows }
        }
    }

    fn a_txid() -> TxId {
        TxId::from_bytes([7u8; 32])
    }

    /// A no-op sleep so retry/probe cadence costs no real time in tests.
    fn no_sleep(_: Duration) -> std::future::Ready<()> {
        std::future::ready(())
    }

    #[tokio::test]
    async fn accepts_on_first_submit() {
        let target = ScriptedTarget::new(vec![Ok("server-txid".into())], false);
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("accepted");
        assert_eq!(out, "server-txid");
        assert_eq!(target.submit_calls(), 1);
        assert_eq!(target.knows_calls(), 0);
    }

    #[tokio::test]
    async fn duplicate_in_mempool_is_success() {
        let target = ScriptedTarget::new(
            vec![Err("error: transaction already exists in mempool".into())],
            false,
        );
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("duplicate counts as delivered");
        assert_eq!(out, a_txid().to_string());
        assert_eq!(target.submit_calls(), 1, "no retry on a duplicate");
    }

    #[tokio::test]
    async fn already_in_chain_is_success() {
        let target = ScriptedTarget::new(
            vec![Err("transaction already in block chain".into())],
            false,
        );
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("mined duplicate counts as delivered");
        assert_eq!(out, a_txid().to_string());
    }

    #[tokio::test]
    async fn queued_probe_settles_into_acceptance() {
        let target = ScriptedTarget::new(
            vec![
                Err("already queued for download".into()),
                Err("already queued for download".into()),
                Ok("server-txid".into()),
            ],
            false,
        );
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("queued then accepted");
        assert_eq!(out, "server-txid");
        assert_eq!(target.submit_calls(), 3, "two probes then acceptance");
    }

    #[tokio::test]
    async fn queued_probe_exhausts() {
        let submits = std::iter::repeat_n(
            Err::<String, String>("already queued for download".into()),
            (MAX_QUEUED_PROBES as usize) + 1,
        )
        .collect();
        let target = ScriptedTarget::new(submits, false);
        let err = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect_err("probes exhausted");
        assert!(err.0.contains("already queued for download"));
        // One initial submit plus MAX_QUEUED_PROBES probes.
        assert_eq!(target.submit_calls(), (MAX_QUEUED_PROBES as usize) + 1);
        assert_eq!(
            target.knows_calls(),
            0,
            "queued exhaustion is not a delivery check"
        );
    }

    #[tokio::test]
    async fn transient_errors_retry_then_succeed() {
        let target = ScriptedTarget::new(
            vec![
                Err("connection reset".into()),
                Err("connection reset".into()),
                Ok("server-txid".into()),
            ],
            false,
        );
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("retried then accepted");
        assert_eq!(out, "server-txid");
        assert_eq!(target.submit_calls(), 3);
    }

    #[tokio::test]
    async fn retry_exhaustion_confirmed_by_delivery_check() {
        // MAX_RETRIES + 1 transient failures; the server knows the txid, so a
        // lost earlier response is treated as delivered.
        let submits = std::iter::repeat_n(
            Err::<String, String>("timeout".into()),
            (MAX_RETRIES as usize) + 1,
        )
        .collect();
        let target = ScriptedTarget::new(submits, true);
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect("delivery check rescues a lost response");
        assert_eq!(out, a_txid().to_string());
        assert_eq!(target.knows_calls(), 1);
    }

    #[tokio::test]
    async fn retry_exhaustion_denied_by_delivery_check_fails() {
        let submits = std::iter::repeat_n(
            Err::<String, String>("timeout".into()),
            (MAX_RETRIES as usize) + 1,
        )
        .collect();
        let target = ScriptedTarget::new(submits, false);
        let err = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep)
            .await
            .expect_err("server does not know it, so it failed");
        assert_eq!(err.0, "timeout");
        assert_eq!(target.knows_calls(), 1);
    }

    /// Pins the classification table directly, so the seam that zaino#1392's
    /// typed rejections will replace has its own contract tests.
    #[test]
    fn rejection_messages_classify_by_the_pinned_table() {
        let table = [
            (
                "error: transaction already exists in mempool",
                RejectionClass::StorageBackedDuplicate,
            ),
            (
                "transaction already in block chain",
                RejectionClass::StorageBackedDuplicate,
            ),
            (
                "tx already queued for download",
                RejectionClass::QueuedProbe,
            ),
            ("connection reset", RejectionClass::Transient),
            ("timeout", RejectionClass::Transient),
            ("", RejectionClass::Transient),
        ];
        for (message, expected) in table {
            assert_eq!(
                classify_rejection(message),
                expected,
                "message: {message:?}"
            );
        }
    }

    /// A duplicate verdict wins over a queued verdict when both substrings
    /// appear, mirroring the check order the inline policy always had.
    #[test]
    fn duplicate_outranks_queued_when_both_substrings_appear() {
        assert_eq!(
            classify_rejection(
                "transaction already exists in mempool and already queued for download"
            ),
            RejectionClass::StorageBackedDuplicate
        );
    }
}
