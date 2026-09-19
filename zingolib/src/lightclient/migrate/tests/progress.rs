use crate::lightclient::migrate::{
    BatchPhase, ImmediateMigrationPhase, MigrationProgress, MigrationProgressHandle,
    PreparationPhase,
};

#[test]
fn the_progress_handle_tracks_each_kind_of_work() {
    let handle = MigrationProgressHandle::default();
    let observer = handle.subscribe();
    assert_eq!(*observer.borrow(), MigrationProgress::Idle);

    handle.begin_immediate(4);
    match &*observer.borrow() {
        MigrationProgress::Immediate(status) => {
            assert_eq!((status.total, status.built, status.sent), (4, 0, 0));
            assert_eq!(status.phase, ImmediateMigrationPhase::Building);
        }
        other => panic!("armed by begin_immediate, got {other:?}"),
    }
    handle.set_built(2);
    handle.enter_transmit();
    handle.set_sent(3);
    handle.resolve(9, 9);
    match &*observer.borrow() {
        MigrationProgress::Immediate(status) => {
            assert_eq!((status.built, status.sent), (2, 3));
            assert_eq!(status.phase, ImmediateMigrationPhase::Transmitting);
        }
        other => panic!("still an immediate migration, got {other:?}"),
    }
    handle.clear();
    assert_eq!(*observer.borrow(), MigrationProgress::Idle);

    handle.begin_preparation(16);
    handle.set_built(7);
    handle.enter_transmit();
    handle.set_sent(3);
    match &*observer.borrow() {
        MigrationProgress::Preparing(status) => {
            assert_eq!((status.total, status.built, status.sent), (16, 7, 3));
            assert_eq!(status.phase, PreparationPhase::Transmitting);
        }
        other => panic!("armed by begin_preparation, got {other:?}"),
    }
    handle.clear();

    handle.begin_batch(3);
    handle.resolve(2, 1);
    handle.set_phase(BatchPhase::Spacing);
    handle.set_built(5);
    match &*observer.borrow() {
        MigrationProgress::Sending(status) => {
            assert_eq!((status.total, status.resolved, status.sent), (3, 2, 1));
            assert_eq!(status.phase, BatchPhase::Spacing);
        }
        other => panic!("armed by begin_batch, got {other:?}"),
    }
    handle.clear();
    assert_eq!(*observer.borrow(), MigrationProgress::Idle);

    handle.set_built(9);
    handle.set_sent(9);
    handle.enter_transmit();
    handle.resolve(1, 1);
    assert_eq!(
        *observer.borrow(),
        MigrationProgress::Idle,
        "mutators are inert while idle"
    );
}

#[tokio::test]
async fn a_batch_publishes_its_progress_and_returns_to_idle() {
    use std::time::Duration;

    use zcash_primitives::transaction::TxId;

    use super::fixtures::{
        TIP, current_bucket_of, scheduled_state, signed_transfer, wallet_with_notes, window_end_of,
    };
    use crate::lightclient::LightClient;
    use crate::wallet::migration::MigrationParams;

    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let transfers = (0..2)
        .map(|index| {
            signed_transfer(
                index,
                notes[index as usize],
                current_bucket,
                TxId::from_bytes([index as u8 + 1; 32]),
                window_end,
                Some(vec![index as u8; 64]),
            )
        })
        .collect();
    wallet.migration = Some(scheduled_state(params, transfers));
    let mut client = LightClient::new_for_test(wallet).await;
    let peeking = ProgressPeekingClient {
        observer: client.migration_progress(),
        seen: std::sync::Mutex::new(Vec::new()),
    };

    let report = client
        .broadcast_due_transfers_with(&peeking, Duration::ZERO)
        .await
        .expect("the batch runs");
    assert_eq!(report.sent_txids().len(), 2);
    assert_eq!(
        *client.migration_progress().borrow(),
        MigrationProgress::Idle,
        "the channel returns to idle when the batch ends"
    );

    let seen = peeking.seen.into_inner().expect("seen mutex");
    assert_eq!(seen.len(), 2, "one snapshot per submission");
    let [
        MigrationProgress::Sending(first),
        MigrationProgress::Sending(second),
    ] = &seen[..]
    else {
        panic!("every submission sees a running batch: {seen:?}");
    };
    assert_eq!((first.total, first.resolved, first.sent), (2, 0, 0));
    assert_eq!(first.phase, BatchPhase::Sending);
    assert_eq!((second.total, second.resolved, second.sent), (2, 1, 1));
    assert_eq!(second.phase, BatchPhase::Sending);
}

struct ProgressPeekingClient {
    observer: tokio::sync::watch::Receiver<MigrationProgress>,
    seen: std::sync::Mutex<Vec<MigrationProgress>>,
}

impl crate::wallet::migration::BroadcastClient for ProgressPeekingClient {
    async fn submit(
        &self,
        _raw_tx: Vec<u8>,
        _expiry_height: zcash_protocol::consensus::BlockHeight,
    ) -> Result<
        crate::wallet::migration::BroadcastReceipt,
        crate::wallet::migration::TransferBroadcastError,
    > {
        self.seen
            .lock()
            .expect("seen mutex")
            .push(self.observer.borrow().clone());
        Ok(crate::wallet::migration::BroadcastReceipt {
            txid: crate::mocks::default_txid(),
            route: crate::wallet::migration::BroadcastRoute::Clearnet {
                endpoint: "peek.example".to_string(),
            },
        })
    }
}
