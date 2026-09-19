use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::fixtures::{
    NOTE_VALUE, current_bucket_of, scheduled_state, signed_transfer, wallet_with_migration_note,
    window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::migrate::{TransferBroadcastResult, TransferOutcome};
use crate::lightclient::transmit::MAX_QUEUED_PROBES;
use crate::wallet::LightWallet;
use crate::wallet::migration::{
    BroadcastClient, BroadcastReceipt, BroadcastRoute, MigrationParams, TransferBroadcastError,
    TransferId, TransferState,
};

const OPEN_TIP: u32 = 300;

const QUEUED_MESSAGE: &str = "transaction dropped because it is already queued for download";

fn own_txid() -> TxId {
    TxId::from_bytes([7; 32])
}

fn route() -> BroadcastRoute {
    BroadcastRoute::Clearnet {
        endpoint: "queued.example".to_string(),
    }
}

struct QueuedProbeClient {
    queued_answers: usize,
    submissions: AtomicUsize,
}

impl QueuedProbeClient {
    fn answering_queued(times: usize) -> Self {
        QueuedProbeClient {
            queued_answers: times,
            submissions: AtomicUsize::new(0),
        }
    }

    fn submissions(&self) -> usize {
        self.submissions.load(Ordering::SeqCst)
    }
}

impl BroadcastClient for QueuedProbeClient {
    async fn submit(
        &self,
        _raw_tx: Vec<u8>,
        _expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        let answered = self.submissions.fetch_add(1, Ordering::SeqCst);
        if answered < self.queued_answers {
            return Err(TransferBroadcastError::Rejected {
                route: route(),
                message: QUEUED_MESSAGE.to_string(),
            });
        }
        Ok(BroadcastReceipt {
            txid: own_txid(),
            route: route(),
        })
    }
}

fn wallet_with_a_signed_open_window_transfer() -> LightWallet {
    let (mut wallet, bound_note) = wallet_with_migration_note(OPEN_TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket,
        own_txid(),
        window_end,
        Some(vec![0xAB; 64]),
    );
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    wallet
}

async fn client_with_instant_probes() -> LightClient {
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer()).await;
    client.set_transmit_retry_interval(Duration::ZERO);
    client
}

fn sent_outcome() -> Vec<TransferOutcome> {
    vec![TransferOutcome {
        transfer: TransferId(0),
        denomination: NOTE_VALUE,
        result: TransferBroadcastResult::Sent(BroadcastReceipt {
            txid: own_txid(),
            route: route(),
        }),
    }]
}

#[tokio::test]
async fn a_submission_queued_for_download_is_probed_until_the_endpoint_accepts_it() {
    const QUEUED_ANSWERS: usize = 3;
    let mut client = client_with_instant_probes().await;
    let endpoint = QueuedProbeClient::answering_queued(QUEUED_ANSWERS);

    let report = client
        .broadcast_due_transfers_with(&endpoint, Duration::ZERO)
        .await
        .expect("the batch runs");

    assert_eq!(report.outcomes, sent_outcome());
    assert_eq!(report.halted, None);
    assert_eq!(
        endpoint.submissions(),
        QUEUED_ANSWERS + 1,
        "one probe per queued answer, then the accepted submission"
    );
    let transfer = client
        .wallet()
        .read()
        .await
        .migration
        .as_ref()
        .expect("stands")
        .transfers[0]
        .clone();
    assert_eq!(transfer.state, TransferState::Broadcast);
    assert_eq!(transfer.attempts, 1, "the probes are one attempt");
}

#[tokio::test]
async fn a_submission_queued_past_the_probe_budget_counts_as_sent() {
    let mut client = client_with_instant_probes().await;
    let endpoint = QueuedProbeClient::answering_queued(usize::MAX);

    let report = client
        .broadcast_due_transfers_with(&endpoint, Duration::ZERO)
        .await
        .expect("the batch runs");

    assert_eq!(
        report.outcomes,
        sent_outcome(),
        "a transaction the endpoint keeps queued was delivered"
    );
    assert_eq!(report.halted, None);
    assert_eq!(
        endpoint.submissions(),
        usize::from(MAX_QUEUED_PROBES) + 1,
        "the first submission plus the whole probe budget"
    );
    assert_eq!(
        client
            .wallet()
            .read()
            .await
            .migration
            .as_ref()
            .expect("stands")
            .transfers[0]
            .state,
        TransferState::Broadcast
    );
}

#[tokio::test]
async fn a_rejection_that_is_not_a_queued_probe_halts_at_once() {
    struct RejectingClient {
        submissions: AtomicUsize,
    }

    impl BroadcastClient for RejectingClient {
        async fn submit(
            &self,
            _raw_tx: Vec<u8>,
            _expiry_height: BlockHeight,
        ) -> Result<BroadcastReceipt, TransferBroadcastError> {
            self.submissions.fetch_add(1, Ordering::SeqCst);
            Err(TransferBroadcastError::Rejected {
                route: route(),
                message: "bad-txns-sapling-binding-signature-invalid".to_string(),
            })
        }
    }

    let mut client = client_with_instant_probes().await;
    let endpoint = RejectingClient {
        submissions: AtomicUsize::new(0),
    };

    let report = client
        .broadcast_due_transfers_with(&endpoint, Duration::ZERO)
        .await
        .expect("the batch runs");

    let halted = report.halted.expect("a rejection halts the batch");
    assert!(halted.contains("bad-txns"), "{halted}");
    assert_eq!(
        endpoint.submissions.load(Ordering::SeqCst),
        1,
        "a real rejection is never probed"
    );
    assert_eq!(
        client
            .wallet()
            .read()
            .await
            .migration
            .as_ref()
            .expect("stands")
            .transfers[0]
            .state,
        TransferState::Signed
    );
}
