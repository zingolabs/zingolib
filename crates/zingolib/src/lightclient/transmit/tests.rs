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
        classify_rejection("transaction already exists in mempool and already queued for download"),
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
