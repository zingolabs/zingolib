//! The resilient transmission policy: retry, duplicate-in-mempool, and
//! queued-probe handling, defined once and generic over the transmission
//! target so the clearnet indexer path and the Nym transmission path share a
//! single implementation.
//!
//! [`resilient_transmit`] performs no wallet-state mutation: it interprets a
//! target's responses and returns the server-reported txid or a
//! [`TransmitFailed`]. The caller owns any wallet effects. The probe/retry
//! cadence is injected as a `sleep` hook so the policy is unit-tested without
//! real time.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zcash_primitives::transaction::TxId;

/// Transient-error retries before the delivery check.
pub(crate) const MAX_RETRIES: u8 = 3;

/// A "queued for download" duplicate rejection proves delivery but not
/// minability: zebra is still verifying the earlier submission (observed to
/// lag it by seconds under load). Each resubmission is a free probe of zebra's
/// own state. Wait up to this many probes, on the retry loop's one-second
/// cadence, for the verdict to become storage-backed (issue #2450).
pub(crate) const MAX_QUEUED_PROBES: u8 = 30;

use zingo_netutils::time::TRANSMIT_RETRY_INTERVAL;

/// A shareable snapshot of the in-flight Transmission's latest progress line,
/// or `None` when no transmission is running. A consumer holding a clone (the
/// CLI's heartbeat, a UI) polls [`Self::latest`] while the transmitting call
/// holds `&mut LightClient`. The transmit path updates it as submissions,
/// retries, probes, and escalation rounds occur. Mirrors the
/// `ImmediateMigrationProgressHandle` pattern.
#[derive(Clone, Debug, Default)]
pub struct TransmitProgressHandle(Arc<Mutex<Option<String>>>);

impl TransmitProgressHandle {
    /// The latest progress line, or `None` when no transmission is running.
    pub fn latest(&self) -> Option<String> {
        self.0
            .lock()
            .expect("transmit progress mutex poisoned")
            .clone()
    }

    /// Publishes `line` as the latest progress snapshot.
    pub(crate) fn set(&self, line: String) {
        *self.0.lock().expect("transmit progress mutex poisoned") = Some(line);
    }

    /// Clears the snapshot. Polling consumers read "no transmission running".
    pub(crate) fn clear(&self) {
        *self.0.lock().expect("transmit progress mutex poisoned") = None;
    }
}

/// Clears the progress snapshot on every exit (success, `?`-propagated
/// error, or panic) so a finished transmission never leaves a stale line.
pub(crate) struct TransmitProgressScope(pub(crate) TransmitProgressHandle);

impl Drop for TransmitProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// What the resilience policy reads from a target's typed failure. The
/// failure value itself travels whole — nothing is flattened to a string on
/// the way through the policy — and this trait exposes the one text the
/// policy classifies: the server's own verdict, when there is one.
pub(crate) trait SubmitFailure: std::fmt::Display {
    /// The server's verdict text (a rejection message or an RPC status
    /// message), or `None` for a transport failure that carries no verdict.
    fn rejection_text(&self) -> Option<&str>;
}

/// The clearnet path's failure type: a gRPC status, carried whole. The
/// server's verdict is the status message (rejections are folded into a
/// status by `GrpcIndexer::send_transaction`).
impl SubmitFailure for zingo_netutils::Status {
    fn rejection_text(&self) -> Option<&str> {
        Some(self.message())
    }
}

/// The mixnet path's failure type, carried whole. Only the variants that
/// hold a server verdict offer text to classify; transport failures are
/// transient by construction.
#[cfg(feature = "nym")]
impl SubmitFailure for zingo_netutils::Socks5TransmitError {
    fn rejection_text(&self) -> Option<&str> {
        match self {
            zingo_netutils::Socks5TransmitError::Rejected(rejection) => Some(&rejection.message),
            zingo_netutils::Socks5TransmitError::Rpc { status, .. } => Some(status.message()),
            _ => None,
        }
    }
}

/// A single transmission endpoint: submits a serialized transaction and can
/// ask the server whether it already knows a txid. Implemented for the
/// configured clearnet indexer and for a Nym Destination reached
/// through the SOCKS5 proxy.
pub(crate) trait TransmitTarget {
    /// The target's typed failure, preserved whole through the policy.
    type Failure: SubmitFailure + Send;

    /// Submit `raw_tx` at `height`, returning the server-reported txid on
    /// acceptance, or the typed failure. Its
    /// [`rejection_text`](SubmitFailure::rejection_text) is classified by
    /// [`resilient_transmit`] (duplicate, queued, or transient).
    fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> impl Future<Output = Result<String, Self::Failure>> + Send;

    /// Whether the server knows `txid`, a delivery check run after the
    /// retries are exhausted, since a lost response can mask a received
    /// transaction.
    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send;
}

/// The transmission exhausted the resilience policy: retries or queued-probes
/// ran out and the server does not know the transaction. Carries the last
/// typed failure, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransmitFailed<F>(pub F);

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
/// (zingolabs/zaino#1392). When that lands, typed checks replace this
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

/// Submit `raw_tx` to `target` under the shared resilience policy, the single
/// definition of the retry / duplicate-in-mempool / queued-probe behavior.
///
/// A duplicate already in the mempool or chain counts as success (an earlier
/// submission is minable or mined). "Queued for download" is re-probed up to
/// [`MAX_QUEUED_PROBES`] times until the verdict is storage-backed. Any other
/// error retries up to [`MAX_RETRIES`] times. On either exhaustion a delivery
/// check ([`TransmitTarget::knows_transaction`]) confirms whether an earlier
/// attempt was in fact received. Returns the server-reported txid on success.
///
/// `sleep` supplies the wait between probes/retries. Production passes
/// `tokio::time::sleep`, tests pass a no-op so the policy runs instantly.
/// `report` receives a succinct line at each state change (submitting,
/// retrying, probing, delivery-checking) for progress display. The caller
/// prefixes it with the target's identity.
pub(crate) async fn resilient_transmit<T, S, F, P>(
    target: &T,
    raw_tx: &[u8],
    height: u64,
    txid: &TxId,
    mut sleep: S,
    report: P,
) -> Result<String, TransmitFailed<T::Failure>>
where
    T: TransmitTarget + Sync,
    S: FnMut(Duration) -> F,
    F: Future<Output = ()>,
    P: Fn(String),
{
    let mut retry_count: u8 = 0;
    let mut queued_probes: u8 = 0;

    report("submitting".to_string());
    let exhausted = loop {
        let failure = match target.submit(raw_tx, height).await {
            Ok(server_txid) => return Ok(server_txid),
            Err(failure) => failure,
        };

        match classify_rejection(failure.rejection_text().unwrap_or_default()) {
            RejectionClass::StorageBackedDuplicate => return Ok(txid.to_string()),
            RejectionClass::QueuedProbe => {
                if queued_probes >= MAX_QUEUED_PROBES {
                    break failure;
                }
                queued_probes += 1;
                report(format!(
                    "delivered, awaiting the server's verdict (probe {queued_probes}/{MAX_QUEUED_PROBES})"
                ));
                sleep(TRANSMIT_RETRY_INTERVAL).await;
            }
            RejectionClass::Transient => {
                if retry_count >= MAX_RETRIES {
                    break failure;
                }
                retry_count += 1;
                report(format!(
                    "retrying after a transient error (retry {retry_count}/{MAX_RETRIES})"
                ));
                sleep(TRANSMIT_RETRY_INTERVAL).await;
            }
        }
    };

    // A transmission error does not prove the transaction failed to reach
    // the network: an earlier attempt may have been accepted with its
    // response lost, and a queued verdict may become storage-backed moments
    // after the last probe. Only fail if the server does not know it.
    report("checking whether an earlier attempt was delivered".to_string());
    if target.knows_transaction(txid).await {
        return Ok(txid.to_string());
    }
    Err(TransmitFailed(exhausted))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A scripted failure whose whole text is the server's verdict, so the
    /// policy classifies exactly what the script says.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestFailure(String);

    impl std::fmt::Display for TestFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl SubmitFailure for TestFailure {
        fn rejection_text(&self) -> Option<&str> {
            Some(&self.0)
        }
    }

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
        type Failure = TestFailure;

        fn submit(
            &self,
            _raw_tx: &[u8],
            _height: u64,
        ) -> impl Future<Output = Result<String, TestFailure>> + Send {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            self.submit_calls.fetch_add(1, Ordering::Relaxed);
            let result = self
                .submits
                .get(i)
                .cloned()
                .unwrap_or_else(|| Err("script exhausted".to_string()))
                .map_err(TestFailure);
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
            .await
            .expect("queued then accepted");
        assert_eq!(out, "server-txid");
        assert_eq!(target.submit_calls(), 3, "two probes then acceptance");
    }

    #[tokio::test]
    async fn queued_probe_exhaustion_runs_the_delivery_check() {
        let submits = std::iter::repeat_n(
            Err::<String, String>("already queued for download".into()),
            (MAX_QUEUED_PROBES as usize) + 1,
        )
        .collect();
        let target = ScriptedTarget::new(submits, false);
        let err = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
            .await
            .expect_err("probes exhausted and the server does not know the txid");
        assert!(err.0.0.contains("already queued for download"));
        // One initial submit plus MAX_QUEUED_PROBES probes.
        assert_eq!(target.submit_calls(), (MAX_QUEUED_PROBES as usize) + 1);
        assert_eq!(
            target.knows_calls(),
            1,
            "queued exhaustion ends with the delivery check"
        );
    }

    /// HYPOTHESIS: a verdict that lands moments after the last queued probe
    /// is still a success — the delivery check converts the exhaustion into
    /// the acceptance the server reached (the 2026-08-06 container run,
    /// where the mempool held the transaction nine seconds after the first
    /// submission). Falsified if exhaustion fails without consulting the
    /// server.
    #[tokio::test]
    async fn queued_exhaustion_with_a_known_transaction_is_success() {
        let submits = std::iter::repeat_n(
            Err::<String, String>("already queued for download".into()),
            (MAX_QUEUED_PROBES as usize) + 1,
        )
        .collect();
        let target = ScriptedTarget::new(submits, true);
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
            .await
            .expect("the server knows the transaction, so delivery stands");
        assert_eq!(out, a_txid().to_string());
        assert_eq!(target.knows_calls(), 1);
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
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
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
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
        let err = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |_| ())
            .await
            .expect_err("server does not know it, so it failed");
        assert_eq!(err.0, TestFailure("timeout".to_string()));
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

    /// Falsifier for the progress narration: a transient failure followed by a
    /// queued rejection and a final acceptance must narrate each state change
    /// in order (submit, retry, probe) so a heartbeat consumer always holds
    /// a line describing what the policy is actually doing.
    #[tokio::test]
    async fn narrates_each_state_change_in_order() {
        let target = ScriptedTarget::new(
            vec![
                Err("connection refused".into()),
                Err("already queued for download".into()),
                Ok("server-txid".into()),
            ],
            false,
        );
        let lines = std::sync::Mutex::new(Vec::new());
        let out = resilient_transmit(&target, b"tx", 1, &a_txid(), no_sleep, |line| {
            lines.lock().expect("narration mutex poisoned").push(line);
        })
        .await
        .expect("accepted on the third submit");
        assert_eq!(out, "server-txid");
        assert_eq!(
            *lines.lock().expect("narration mutex poisoned"),
            vec![
                "submitting".to_string(),
                format!("retrying after a transient error (retry 1/{MAX_RETRIES})"),
                format!("delivered, awaiting the server's verdict (probe 1/{MAX_QUEUED_PROBES})"),
            ]
        );
    }

    /// The progress handle round-trips a line and clears through its scope on
    /// every exit path, so no stale line outlives a transmission.
    #[test]
    fn progress_handle_sets_and_scope_clears() {
        let handle = TransmitProgressHandle::default();
        assert_eq!(handle.latest(), None);
        handle.set("submitting".to_string());
        assert_eq!(handle.latest(), Some("submitting".to_string()));
        {
            let _scope = TransmitProgressScope(handle.clone());
        }
        assert_eq!(handle.latest(), None, "the scope's drop clears the line");
    }
}
