//! The resilient transmission policy: retry, duplicate-in-mempool, and
//! queued-probe handling, defined once and generic over the transmission
//! target so the nakednet indexer path and the Nym transmission path share a
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

use zl_netutils::time::TRANSMIT_RETRY_INTERVAL;

/// A shareable snapshot of the in-flight Transmission's latest progress line,
/// or `None` when no transmission is running. A consumer holding a clone (the
/// CLI's heartbeat, a UI) polls [`Self::latest`] while the transmitting call
/// holds `&mut LightClient`. The transmit path updates it as submissions,
/// retries, and probes occur. Mirrors the
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

/// The nakednet path's failure type: a gRPC status, carried whole. The
/// server's verdict is the status message (rejections are folded into a
/// status by `GrpcIndexer::send_transaction`).
impl SubmitFailure for zl_netutils::Status {
    fn rejection_text(&self) -> Option<&str> {
        Some(self.message())
    }
}

/// The mixnet path's failure type, carried whole. Only the variants that
/// hold a server verdict offer text to classify; transport failures are
/// transient by construction.
#[cfg(feature = "nym")]
impl SubmitFailure for zl_netutils::Socks5TransmitError {
    fn rejection_text(&self) -> Option<&str> {
        match self {
            zl_netutils::Socks5TransmitError::Rejected(rejection) => Some(&rejection.message),
            zl_netutils::Socks5TransmitError::Rpc { status, .. } => Some(status.message()),
            _ => None,
        }
    }
}

/// A single transmission endpoint: submits a serialized transaction and can
/// ask the server whether it already knows a txid. Implemented for the
/// configured nakednet indexer and for a Nym Destination reached
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
mod tests;
