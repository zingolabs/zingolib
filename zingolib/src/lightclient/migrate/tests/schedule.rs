use zcash_primitives::transaction::TxId;

use super::fixtures::{
    FAR_EXPIRY, FUNDING_NOTE, NOTE_VALUE, RESIDUAL_NOTE, SEED, TIP, UNPREPARED_NOTE, bound_note_of,
    committed_client, current_bucket_of, mark_note_spent_by, migration_error, migration_of,
    phase_of, scheduled_client, scheduled_state, signed_transfer, sorted,
    wallet_with_funding_notes, wallet_with_migration_note,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::lightclient::migrate::ProposedSchedule;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::migration::{MigrationParams, MigrationPhase, TransferState, schedule};

#[tokio::test]
async fn propose_schedule_refuses_before_preparation_is_complete() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let client = committed_client(wallet).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Committed);

    let result = client.propose_schedule(1).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NotPrepared
    ));
}

#[tokio::test]
async fn propose_schedule_refuses_without_a_migration() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let client = LightClient::new_for_test(wallet).await;
    let result = client.propose_schedule(1).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));
}

#[tokio::test]
async fn propose_schedule_lists_every_funding_note_in_a_window_at_or_after_the_current_one() {
    let wallet = wallet_with_funding_notes(TIP, 3);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let client = committed_client(wallet).await;

    let proposed = client
        .propose_schedule(1)
        .await
        .expect("a prepared migration proposes");

    assert_eq!(proposed.transfers_per_window, 1);
    assert_eq!(proposed.transfers.len(), 3, "one transfer per funding note");
    for transfer in &proposed.transfers {
        assert_eq!(transfer.denomination, NOTE_VALUE);
        assert!(
            transfer.window >= current_bucket,
            "window {} is not before the current bucket {current_bucket}",
            transfer.window
        );
        assert_eq!(
            transfer.boundary,
            schedule::boundary_of(transfer.window, params.bucket_modulus)
        );
        let close = schedule::boundary_of(transfer.window + 1, params.bucket_modulus);
        assert!(
            transfer.boundary <= transfer.scheduled_broadcast_height
                && transfer.scheduled_broadcast_height < close,
            "the scheduled broadcast height {} lies inside window {}",
            transfer.scheduled_broadcast_height,
            transfer.window
        );
    }
    let windows: Vec<u64> = proposed.transfers.iter().map(|t| t.window).collect();
    assert_eq!(
        windows,
        vec![current_bucket, current_bucket + 1, current_bucket + 2],
        "one transfer per window fills consecutive windows from the current one"
    );
    assert!(
        migration_of(&client).await.transfers.is_empty(),
        "proposing stores nothing"
    );
}

#[tokio::test]
async fn propose_schedule_clamps_zero_transfers_per_window_to_one() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let client = committed_client(wallet).await;
    let proposed = client.propose_schedule(0).await.expect("proposes");
    assert_eq!(proposed.transfers_per_window, 1);
}

#[tokio::test]
async fn commit_schedule_stores_exactly_the_proposed_windows_and_anchors() {
    let wallet = wallet_with_funding_notes(TIP, 3);
    let mut client = committed_client(wallet).await;
    let proposed = client.propose_schedule(1).await.expect("proposes");

    client
        .commit_schedule(&proposed)
        .await
        .expect("the proposal commits");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Scheduled);
    assert_eq!(state.params.k_max, 1, "the cadence is recorded");
    assert_eq!(
        state.commitment.params_hash,
        state.params.params_hash(),
        "the commitment covers the recorded cadence"
    );
    assert_eq!(state.transfers.len(), proposed.transfers.len());
    for (record, transfer) in state.transfers.iter().zip(&proposed.transfers) {
        assert_eq!(record.state, TransferState::Assigned);
        assert_eq!(record.denomination, transfer.denomination);
        assert_eq!(record.bucket_index, Some(transfer.window));
        assert_eq!(
            record.target_height,
            Some(transfer.scheduled_broadcast_height)
        );
        assert!(
            record
                .anchor_bucket
                .is_some_and(|anchor| anchor < transfer.window),
            "the anchor is drawn below the window"
        );
        assert_eq!(
            record.note,
            Some(transfer.note),
            "the funding note is bound"
        );
        assert_eq!(record.anchor_bucket, Some(transfer.anchor_bucket));
    }
}

#[tokio::test]
async fn commit_schedule_reserves_only_the_funding_notes() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(RESIDUAL_NOTE)
        .tip(TIP)
        .build();
    let funding = bound_note_of(&wallet, FUNDING_NOTE);
    let residual = bound_note_of(&wallet, RESIDUAL_NOTE);
    let mut client = committed_client(wallet).await;
    assert_eq!(
        sorted(client.wallet().read().await.reserved_output_ids()),
        sorted(vec![funding.output_id, residual.output_id]),
        "before the schedule every note is reserved"
    );
    let proposed = client.propose_schedule(1).await.expect("proposes");

    client
        .commit_schedule(&proposed)
        .await
        .expect("the proposal commits");

    let wallet = client.wallet().read().await;
    assert_eq!(
        wallet.reserved_output_ids(),
        vec![funding.output_id],
        "only the funding note stays reserved"
    );
    let summaries = wallet.note_summaries::<pepper_sync::wallet::OrchardNote>(false);
    let residual_summary = summaries
        .iter()
        .find(|note| note.value == RESIDUAL_NOTE)
        .expect("the residual note is listed");
    assert!(!residual_summary.reserved, "the residual note is free");
    let funding_summary = summaries
        .iter()
        .find(|note| note.value == FUNDING_NOTE)
        .expect("the funding note is listed");
    assert!(funding_summary.reserved);
}

#[tokio::test]
async fn commit_schedule_refuses_when_a_funding_note_was_spent_since_the_proposal() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let spent = bound_note_of(&wallet, FUNDING_NOTE);
    let mut client = committed_client(wallet).await;
    let proposed = client.propose_schedule(1).await.expect("proposes");
    mark_note_spent_by(
        &mut *client.wallet().write().await,
        &spent,
        TxId::from_bytes([9; 32]),
    );

    let result = client.commit_schedule(&proposed).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::ScheduleMismatch
    ));
    let state = migration_of(&client).await;
    assert_eq!(
        state.phase,
        MigrationPhase::Prepared,
        "the refusal writes nothing"
    );
    assert!(state.transfers.is_empty());
}

#[tokio::test]
async fn commit_schedule_refuses_before_preparation_is_complete() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = committed_client(wallet).await;
    let proposed = client.propose_schedule(1).await.expect("proposes");
    client
        .wallet()
        .write()
        .await
        .migration
        .as_mut()
        .expect("the migration exists")
        .phase = MigrationPhase::Committed;

    let result = client.commit_schedule(&proposed).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NotPrepared
    ));
    assert_eq!(phase_of(&client).await, MigrationPhase::Committed);
}

#[tokio::test]
async fn the_schedule_is_fixed_once_a_transfer_is_signed() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket,
        TxId::from_bytes([7; 32]),
        FAR_EXPIRY,
        None,
    );
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;
    let before = migration_of(&client).await;

    let result = client.propose_schedule(2).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::ScheduleFixed
    ));

    let empty = ProposedSchedule {
        transfers_per_window: 1,
        transfers: Vec::new(),
        drawn_at: zcash_protocol::consensus::BlockHeight::from_u32(TIP),
    };
    let result = client.commit_schedule(&empty).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::ScheduleFixed
    ));
    assert_eq!(migration_of(&client).await, before);
}

#[tokio::test]
async fn a_second_proposal_replaces_the_schedule_before_any_signature() {
    const FUNDING_NOTES: usize = 7;
    let wallet = wallet_with_funding_notes(TIP, FUNDING_NOTES);
    let mut client = scheduled_client(wallet, 1).await;
    let first = migration_of(&client).await;
    assert_eq!(first.params.k_max, 1);
    let distinct_windows = |state: &crate::wallet::migration::MigrationState| {
        state
            .transfers
            .iter()
            .filter_map(|transfer| transfer.bucket_index)
            .collect::<std::collections::BTreeSet<u64>>()
            .len()
    };
    assert_eq!(
        distinct_windows(&first),
        FUNDING_NOTES,
        "one transfer per window spreads seven transfers over seven windows"
    );

    let proposed = client
        .propose_schedule(4)
        .await
        .expect("a schedule with no signature is open");
    assert_eq!(proposed.transfers_per_window, 4);
    client
        .commit_schedule(&proposed)
        .await
        .expect("the second proposal commits");

    let second = migration_of(&client).await;
    assert_eq!(second.phase, MigrationPhase::Scheduled);
    assert_eq!(second.params.k_max, 4, "the cadence is replaced");
    for (record, transfer) in second.transfers.iter().zip(&proposed.transfers) {
        assert_eq!(record.bucket_index, Some(transfer.window));
        assert_eq!(record.anchor_bucket, Some(transfer.anchor_bucket));
    }
    assert_eq!(second.transfers.len(), first.transfers.len());
    assert!(
        second
            .transfers
            .iter()
            .all(|transfer| transfer.state == TransferState::Assigned),
        "every transfer is placed afresh"
    );
    assert_eq!(
        distinct_windows(&second),
        4,
        "two transfers per window (seven over six sessions) pack seven transfers into four windows"
    );
}

#[tokio::test]
async fn commit_schedule_records_a_fresh_commit_time() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = committed_client(wallet).await;
    client
        .wallet()
        .write()
        .await
        .migration
        .as_mut()
        .expect("the migration exists")
        .commitment
        .committed_at = 0;
    let proposed = client.propose_schedule(1).await.expect("proposes");
    let plan_hash = migration_of(&client).await.commitment.plan_hash;

    client.commit_schedule(&proposed).await.expect("commits");

    let state = migration_of(&client).await;
    assert!(state.commitment.committed_at > 0);
    assert_eq!(
        state.commitment.plan_hash, plan_hash,
        "the plan commitment is carried over"
    );
}

#[tokio::test]
async fn a_second_proposal_keeps_a_released_transfer_and_never_rebinds_its_note() {
    let wallet = wallet_with_funding_notes(TIP, 3);
    let mut client = scheduled_client(wallet, 1).await;
    let first = migration_of(&client).await;
    let released_note = first.transfers[1].note.expect("bound");
    client
        .release_transfer(crate::wallet::migration::TransferId(1))
        .await
        .expect("a pending transfer releases");

    let proposed = client
        .propose_schedule(2)
        .await
        .expect("a schedule with no signature is open");

    assert_eq!(
        proposed.transfers.len(),
        2,
        "the released note is not proposed again"
    );
    assert!(
        proposed
            .transfers
            .iter()
            .all(|transfer| transfer.note != released_note),
        "the released transfer's note is never rebound"
    );

    client
        .commit_schedule(&proposed)
        .await
        .expect("the second proposal commits");

    let second = migration_of(&client).await;
    assert_eq!(second.transfers.len(), 3, "the released record stays");
    assert_eq!(
        second.transfers[0].state,
        TransferState::Released,
        "released records come first"
    );
    assert_eq!(
        second.transfers[0].id,
        crate::wallet::migration::TransferId(0),
        "released records are renumbered first"
    );
    assert_eq!(second.transfers[0].note, Some(released_note));
    for (index, record) in second.transfers.iter().enumerate().skip(1) {
        assert_eq!(
            record.id,
            crate::wallet::migration::TransferId(index as u32)
        );
        assert_eq!(record.state, TransferState::Assigned);
        assert_ne!(record.note, Some(released_note));
        assert_eq!(record.note, Some(proposed.transfers[index - 1].note));
    }
    let wallet = client.wallet().read().await;
    assert_eq!(wallet.reserved_output_ids().len(), 2);
    assert!(
        !wallet
            .reserved_output_ids()
            .contains(&released_note.output_id),
        "the released note stays free"
    );
}

#[tokio::test]
async fn commit_schedule_refuses_a_proposal_with_a_window_below_the_current_bucket() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let mut client = committed_client(wallet).await;
    let before = migration_of(&client).await;
    let mut proposed = client.propose_schedule(1).await.expect("proposes");
    assert!(
        proposed
            .transfers
            .iter()
            .all(|transfer| transfer.window >= current_bucket),
        "premise: the draw never proposes a closed window"
    );
    let stale = proposed
        .transfers
        .iter_mut()
        .max_by_key(|transfer| transfer.window)
        .expect("two transfers");
    stale.window = current_bucket - 1;
    stale.boundary = schedule::boundary_of(stale.window, params.bucket_modulus);
    stale.anchor_bucket = stale.window - 1;
    stale.scheduled_broadcast_height = stale.boundary;

    let result = client.commit_schedule(&proposed).await;

    assert!(matches!(
        migration_error(result),
        MigrationError::ScheduleMismatch
    ));
    assert_eq!(
        migration_of(&client).await,
        before,
        "the refusal writes nothing"
    );
}

#[tokio::test]
async fn commit_schedule_stores_a_hand_edited_proposal_exactly() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let mut client = committed_client(wallet).await;
    let drawn = client.propose_schedule(1).await.expect("proposes");

    let mut edited = drawn.clone();
    for transfer in edited.transfers.iter_mut() {
        transfer.window += 3;
        transfer.boundary = schedule::boundary_of(transfer.window, params.bucket_modulus);
        transfer.anchor_bucket = transfer.window - 1;
        transfer.scheduled_broadcast_height = transfer.boundary + 5;
    }
    assert_ne!(edited.schedule_hash(), drawn.schedule_hash());

    client
        .commit_schedule(&edited)
        .await
        .expect("a proposal is plain data and commits as given");

    let state = migration_of(&client).await;
    for (record, transfer) in state.transfers.iter().zip(&edited.transfers) {
        assert_eq!(record.bucket_index, Some(transfer.window));
        assert_eq!(record.anchor_bucket, Some(transfer.anchor_bucket));
        assert_eq!(
            record.target_height,
            Some(transfer.scheduled_broadcast_height)
        );
        assert_eq!(record.note, Some(transfer.note));
        assert_eq!(record.denomination, transfer.denomination);
        assert_eq!(record.state, TransferState::Assigned);
        assert!(record.anchor_witness.is_none());
    }
}

#[test]
fn schedule_hash_covers_every_draw_of_the_proposal() {
    let (wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let base = ProposedSchedule {
        transfers_per_window: 2,
        transfers: vec![crate::lightclient::migrate::ProposedTransfer {
            denomination: NOTE_VALUE,
            note: bound_note,
            window: 5,
            boundary: schedule::boundary_of(5, params.bucket_modulus),
            anchor_bucket: 3,
            scheduled_broadcast_height: schedule::boundary_of(5, params.bucket_modulus) + 9,
            scheduled_broadcast_unix_time: 1_800_000_000,
        }],
        drawn_at: zcash_protocol::consensus::BlockHeight::from_u32(TIP),
    };
    assert_eq!(base.schedule_hash(), base.clone().schedule_hash());

    let mut other_window = base.clone();
    other_window.transfers[0].window = 6;
    let mut other_anchor = base.clone();
    other_anchor.transfers[0].anchor_bucket = 2;
    let mut other_target = base.clone();
    other_target.transfers[0].scheduled_broadcast_height =
        base.transfers[0].scheduled_broadcast_height + 1;
    let mut other_cadence = base.clone();
    other_cadence.transfers_per_window = 3;
    let mut other_draw_height = base.clone();
    other_draw_height.drawn_at = zcash_protocol::consensus::BlockHeight::from_u32(TIP - 1);
    let mut other_unix_estimate = base.clone();
    other_unix_estimate.transfers[0].scheduled_broadcast_unix_time += 1;

    let hashes = [
        base.schedule_hash(),
        other_window.schedule_hash(),
        other_anchor.schedule_hash(),
        other_target.schedule_hash(),
        other_cadence.schedule_hash(),
        other_draw_height.schedule_hash(),
    ];
    for (left, first) in hashes.iter().enumerate() {
        for second in &hashes[left + 1..] {
            assert_ne!(first, second, "every draw changes the digest");
        }
    }
    assert_eq!(
        other_unix_estimate.schedule_hash(),
        base.schedule_hash(),
        "the unix estimate is presentation, not part of the draw"
    );
}
