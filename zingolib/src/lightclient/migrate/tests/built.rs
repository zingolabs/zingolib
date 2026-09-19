use std::time::Duration;

use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::fixtures::{
    FUNDING_NOTE, NOTE_VALUE, migration_of, mine_to_and_sync, scheduled_mock_client,
    transaction_status,
};
use crate::lightclient::migrate::{TransferBroadcastResult, TransferOutcome};
use crate::mocks::transmission::MockBroadcastClient;
use crate::wallet::migration::preparation::CANONICAL_TRANSFER_FEE;
use crate::wallet::migration::transfers::verify_canonical_part;
use crate::wallet::migration::{TransferId, TransferState, schedule};

#[tokio::test]
async fn a_transfer_built_for_the_open_window_is_canonical_on_the_wire() {
    let (net, mut client) = scheduled_mock_client(&[FUNDING_NOTE], 1).await;
    let chain_type = net.chain_type();
    let (window, modulus) = {
        let state = migration_of(&client).await;
        (
            state.transfers[0]
                .bucket_index
                .expect("a scheduled transfer has a window"),
            state.params.bucket_modulus,
        )
    };
    let boundary = schedule::boundary_of(window, modulus);
    let target_height = boundary + 1;
    let canonical_expiry = schedule::canonical_expiry_height(target_height);
    mine_to_and_sync(&net, &mut client, u32::from(boundary)).await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, Duration::ZERO)
        .await
        .expect("the open window broadcasts");

    let [
        TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Sent(receipt),
        },
    ] = &report.outcomes[..]
    else {
        panic!("the one transfer is sent: {report:?}");
    };
    let submissions = std::mem::take(&mut *mock.submissions.lock().unwrap());
    let [(raw_tx, expiry_on_the_wire)] = &submissions[..] else {
        panic!("exactly one submission reached the endpoint");
    };
    assert_eq!(
        *expiry_on_the_wire, canonical_expiry,
        "the expiry handed to the endpoint is the canonical expiry of the window's target height"
    );

    let transaction = Transaction::read(
        raw_tx.as_slice(),
        BranchId::for_height(&chain_type, target_height),
    )
    .expect("the submitted bytes are a transaction of the window's branch");
    assert_eq!(transaction.txid(), receipt.txid);
    assert_eq!(
        transaction.expiry_height(),
        canonical_expiry,
        "the transaction commits to the canonical expiry"
    );
    assert_eq!(transaction.lock_time(), 0);
    assert!(transaction.transparent_bundle().is_none());
    assert!(transaction.sapling_bundle().is_none());
    let orchard = transaction
        .orchard_bundle()
        .expect("the transfer spends the funding note in Orchard");
    assert_eq!(
        orchard.actions().len(),
        2,
        "the Orchard bundle is padded to two actions"
    );
    assert_eq!(
        i64::from(orchard.value_balance()),
        i64::try_from(NOTE_VALUE + CANONICAL_TRANSFER_FEE).expect("fits i64"),
        "the Orchard bundle releases the funding note's whole value"
    );
    let ironwood = transaction
        .ironwood_bundle()
        .expect("the transfer pays the denomination into Ironwood");
    assert_eq!(
        ironwood.actions().len(),
        2,
        "the Ironwood bundle is padded to two actions"
    );
    assert_eq!(
        i64::from(ironwood.value_balance()),
        -i64::try_from(NOTE_VALUE).expect("fits i64"),
        "the Ironwood bundle receives exactly the denomination"
    );

    let wallet = client.wallet().read().await;
    let transfer = &wallet.migration.as_ref().expect("stands").transfers[0];
    assert_eq!(transfer.state, TransferState::Broadcast);
    assert_eq!(transfer.txid, Some(transaction.txid()));
    assert_eq!(
        transfer.expiry_height,
        Some(canonical_expiry),
        "the record carries the expiry the build returned"
    );
    assert_eq!(
        transaction_status(&wallet, &transaction.txid()),
        Some(ConfirmationStatus::Calculated(target_height)),
        "the wallet transaction is recorded at the target height the build returned"
    );
    assert_eq!(
        wallet.sync_state.last_known_chain_height(),
        Some(boundary),
        "the broadcast never synchronized"
    );

    let params = wallet.migration.as_ref().expect("stands").params().clone();
    assert!(
        verify_canonical_part(&transaction, NOTE_VALUE, canonical_expiry, &params).is_ok(),
        "the built transfer passes the canonical predicate"
    );
    assert!(
        verify_canonical_part(&transaction, NOTE_VALUE, canonical_expiry + 1, &params).is_err(),
        "an expiry off the canonical one is a deviation"
    );
    assert!(
        verify_canonical_part(&transaction, NOTE_VALUE * 2, canonical_expiry, &params).is_err(),
        "a value balance off the denomination is a deviation"
    );
    let mut off_fee = params.clone();
    off_fee.transfer_fee += 1;
    assert!(
        verify_canonical_part(&transaction, NOTE_VALUE, canonical_expiry, &off_fee).is_err(),
        "a fee off the canonical one is a deviation"
    );
}

#[tokio::test]
async fn a_wallet_that_scanned_the_chain_within_a_window_is_fresh() {
    let (net, mut client) = scheduled_mock_client(&[FUNDING_NOTE], 1).await;
    let (window, modulus) = {
        let state = migration_of(&client).await;
        (
            state.transfers[0]
                .bucket_index
                .expect("a scheduled transfer has a window"),
            state.params.bucket_modulus,
        )
    };
    let boundary = schedule::boundary_of(window, modulus);
    mine_to_and_sync(&net, &mut client, u32::from(boundary)).await;
    let newest_block_time = client
        .wallet()
        .read()
        .await
        .wallet_blocks
        .iter()
        .next_back()
        .map(|(_, block)| block.time())
        .expect("the mock chain's blocks were scanned");
    assert!(
        u64::from(crate::utils::now()).saturating_sub(u64::from(newest_block_time))
            <= u64::from(modulus) * schedule::TARGET_BLOCK_SPACING_SECONDS,
        "premise: the newest block is within one window by the clock"
    );

    let report = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a fresh chain view broadcasts");

    assert!(
        matches!(
            report.outcomes[..],
            [TransferOutcome {
                result: TransferBroadcastResult::Sent(_),
                ..
            }]
        ),
        "{report:?}"
    );
    assert_eq!(net.chain.read().await.mempool_len(), 1);
    assert_eq!(
        migration_of(&client).await.transfers[0].state,
        TransferState::Broadcast
    );
}
