use std::time::Duration;

use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_common_components::protocol::ActivationHeights;
use zip32::AccountId;

use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::migrate::{
    BatchReport, MigrationPlan, TransferBroadcastResult, TransferProgress, TransferStatus,
};
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::{
    LostSendDestination, MockChain, MockNet, pre_ironwood_funding_transaction,
};
use crate::wallet::balance::AccountBalance;
use crate::wallet::error::ProposeSendError;
use crate::wallet::migration::{
    MigrationMode, MigrationPhase, ScheduledMigrationPlan, TransferId, TransferState,
};

use super::external_address;

const SEED: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;

const NU6_3_HEIGHT: u32 = 16;

const FUNDING_HEIGHT: u32 = 4;

const WINDOW: u32 = 144;

const TRANSFER_FEE: u64 = 20_000;

const ONE_MILLION_NOTE: u64 = 1_000_000 + TRANSFER_FEE;

const TWO_MILLION_NOTE: u64 = 2_000_000 + TRANSFER_FEE;

const FIVE_MILLION_NOTE: u64 = 5_000_000 + TRANSFER_FEE;

const RESIDUAL_NOTE: u64 = 9_000;

const UNPREPARED_NOTE: u64 = 3_000_000;

const PREPARATION_FEE: u64 = 15_000;

const PREPARATION_CHANGE: u64 = UNPREPARED_NOTE - TWO_MILLION_NOTE - PREPARATION_FEE;

fn migration_activation_heights() -> ActivationHeights {
    ActivationHeights::builder()
        .set_overwinter(Some(1))
        .set_sapling(Some(1))
        .set_blossom(Some(1))
        .set_heartwood(Some(1))
        .set_canopy(Some(1))
        .set_nu5(Some(2))
        .set_nu6(Some(2))
        .set_nu6_1(Some(5))
        .set_nu6_2(Some(5))
        .set_nu6_3(Some(NU6_3_HEIGHT))
        .set_nu7(None)
        .build()
}

async fn funded_client(note_values: &[u64]) -> (MockNet, LightClient) {
    let heights = migration_activation_heights();
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(heights)).await;
    let mut client = net.client(SEED).await;
    client.consent_to_clearnet_for_tests().await;
    client.set_transmit_retry_interval(Duration::ZERO);

    let address = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let receivers = note_values
        .iter()
        .map(|value| (address.as_str(), *value, None))
        .collect();
    let funding = pre_ironwood_funding_transaction(heights, receivers).await;
    {
        let mut chain = net.chain.write().await;
        let to_funding = FUNDING_HEIGHT - chain.tip() - 1;
        chain.mine_empty_blocks(to_funding);
        chain.mine_block(vec![funding]);
        let past_activation = NU6_3_HEIGHT + 1 - chain.tip();
        chain.mine_empty_blocks(past_activation);
    }
    client
        .sync_and_await()
        .await
        .expect("the funding block scans");
    client
        .sync_and_await()
        .await
        .expect("the second pass completes the spend evidence");
    (net, client)
}

async fn commit_scheduled_plan(client: &mut LightClient) -> ScheduledMigrationPlan {
    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .expect("planning is pure");
    let MigrationPlan::Scheduled(scheduled) = plan.clone() else {
        panic!("a scheduled plan request yields a scheduled plan");
    };
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the scheduled plan commits");
    scheduled
}

async fn commit_schedule_of(client: &mut LightClient, transfers_per_window: u32) {
    let proposed = client
        .propose_schedule(transfers_per_window)
        .await
        .expect("a prepared migration proposes a schedule");
    client
        .commit_schedule(&proposed)
        .await
        .expect("the proposed schedule commits");
}

async fn transfers_by_window(client: &LightClient) -> Vec<TransferStatus> {
    let mut transfers = client
        .migration_status()
        .await
        .expect("the status reads")
        .transfers;
    transfers.sort_by_key(|transfer| (transfer.window, transfer.id));
    transfers
}

async fn phase_of(client: &LightClient) -> Option<MigrationPhase> {
    client
        .migration_status()
        .await
        .expect("the status reads")
        .phase
}

async fn transfer_state(client: &LightClient, id: TransferId) -> TransferState {
    client
        .wallet()
        .read()
        .await
        .migration
        .as_ref()
        .expect("the migration is stored")
        .transfers()[id.0 as usize]
        .state
}

async fn transfer_txid(
    client: &LightClient,
    id: TransferId,
) -> Option<zcash_primitives::transaction::TxId> {
    client
        .wallet()
        .read()
        .await
        .migration
        .as_ref()
        .expect("the migration is stored")
        .transfers()[id.0 as usize]
        .txid
}

async fn mine_to(net: &MockNet, height: u32) {
    let mut chain = net.chain.write().await;
    let tip = chain.tip();
    if tip < height {
        chain.mine_empty_blocks(height - tip);
    }
}

async fn advance_to_window(net: &MockNet, client: &mut LightClient, window: u64) {
    let boundary = u32::try_from(window).expect("window boundaries fit a block height") * WINDOW;
    mine_to(net, boundary).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks up to the window boundary scan");
}

async fn mine_mempool_and_sync(net: &MockNet, client: &mut LightClient) {
    net.chain.write().await.mine_mempool();
    client
        .sync_and_await()
        .await
        .expect("the mined block scans");
}

async fn reconcile_once_more(client: &mut LightClient) {
    client
        .reconcile_migration()
        .await
        .expect("reconciliation applies the safe actions");
}

fn assert_all_sent(report: &BatchReport) {
    assert!(
        report.halted.is_none(),
        "the batch broadcast without halting: {report:?}"
    );
    assert!(
        !report.outcomes.is_empty()
            && report
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome.result, TransferBroadcastResult::Sent(_))),
        "every due transfer was accepted by the endpoint: {report:?}"
    );
}

async fn broadcast_window(net: &MockNet, client: &mut LightClient, window: u64) {
    advance_to_window(net, client, window).await;
    let report = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("the open window broadcasts");
    assert_all_sent(&report);
}

async fn balances(client: &LightClient) -> AccountBalance {
    client
        .account_balance(AccountId::ZERO)
        .await
        .expect("the wallet has view capability")
}

fn zats(balance: Option<Zatoshis>) -> u64 {
    balance
        .expect("the wallet has view capability for this pool")
        .into_u64()
}

#[tokio::test]
async fn a_schedule_of_funding_notes_confirms_every_transfer_and_completes() {
    const DENOMINATIONS: u64 = 1_000_000 + 2_000_000 + 5_000_000;

    let (net, mut client) =
        funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE, FIVE_MILLION_NOTE]).await;
    let ironwood_before = zats(balances(&client).await.confirmed_ironwood_balance);

    let plan = commit_scheduled_plan(&mut client).await;
    assert!(
        plan.is_prepared(),
        "funding notes need no preparation: {plan:?}"
    );
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Prepared),
        "a plan with no preparation rounds commits straight to Prepared"
    );
    commit_schedule_of(&mut client, 1).await;
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Scheduled),
        "the committed schedule leaves the migration scheduled"
    );

    let scheduled = transfers_by_window(&client).await;
    assert_eq!(scheduled.len(), 3, "one transfer per funding note");
    for (position, transfer) in scheduled.iter().enumerate() {
        let window = transfer.window.expect("a scheduled transfer has a window");
        broadcast_window(&net, &mut client, window).await;
        mine_mempool_and_sync(&net, &mut client).await;

        let status = client.migration_status().await.expect("the status reads");
        let confirmed = status
            .transfers
            .iter()
            .find(|candidate| candidate.id == transfer.id)
            .expect("the transfer is still in the schedule");
        assert_eq!(
            confirmed.progress,
            TransferProgress::Confirmed,
            "transfer {position} confirmed once its transaction was mined: {status:?}"
        );
    }

    reconcile_once_more(&mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "every transfer confirmed, so the migration is complete: {status:?}"
    );
    assert_eq!(
        status.value_migrated, DENOMINATIONS,
        "the migrated value is the sum of the denominations"
    );

    let after = balances(&client).await;
    assert_eq!(
        zats(after.confirmed_ironwood_balance),
        ironwood_before + DENOMINATIONS,
        "the Ironwood pool grew by exactly the migrated value"
    );
    assert_eq!(
        zats(after.confirmed_orchard_balance),
        0,
        "the funding notes were sized exactly, so no residual is left in Orchard"
    );
}

#[tokio::test]
async fn the_sync_that_confirms_the_last_transfer_completes_the_migration() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;
    mine_mempool_and_sync(&net, &mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[0].progress,
        TransferProgress::Confirmed,
        "the only transfer confirmed: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the reconciliation pass that confirmed the last transfer also completes the \
         migration, with no further pass needed: {status:?}"
    );
}

#[tokio::test]
async fn a_note_that_needs_preparation_reaches_prepared_and_then_migrates() {
    let (net, mut client) = funded_client(&[UNPREPARED_NOTE, RESIDUAL_NOTE]).await;

    let plan = commit_scheduled_plan(&mut client).await;
    assert_eq!(
        plan.preparation_rounds.len(),
        1,
        "the unprepared note takes one preparation round: {plan:?}"
    );
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Committed),
        "a plan with preparation rounds commits to Committed"
    );

    let round = client
        .broadcast_preparation_round()
        .await
        .expect("the first preparation round broadcasts");
    assert_eq!(round.round, 0, "rounds count from zero");
    assert!(!round.txids.is_empty(), "the round carries transactions");

    let pending = client.broadcast_preparation_round().await;
    assert!(
        matches!(
            pending,
            Err(LightClientError::MigrationError(
                MigrationError::RoundPending { .. }
            ))
        ),
        "a round in flight refuses the next one: {pending:?}"
    );

    mine_mempool_and_sync(&net, &mut client).await;
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Prepared),
        "reconciliation moves a confirmed, anchored round to Prepared"
    );

    let prepared = client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .expect("planning is pure");
    let MigrationPlan::Scheduled(prepared) = prepared else {
        panic!("a scheduled plan request yields a scheduled plan");
    };
    assert_eq!(
        prepared.transfers,
        vec![TWO_MILLION_NOTE - TRANSFER_FEE],
        "the round funded one transfer: {prepared:?}"
    );
    assert_eq!(
        prepared.residual,
        RESIDUAL_NOTE + PREPARATION_CHANGE,
        "the preparation change joins the residual note: {prepared:?}"
    );

    commit_schedule_of(&mut client, 2).await;

    let balance = balances(&client).await;
    assert_eq!(
        zats(balance.reserved_orchard_balance),
        TWO_MILLION_NOTE,
        "the schedule reserves the funding note the round produced"
    );
    assert_eq!(
        zats(balance.confirmed_orchard_balance),
        RESIDUAL_NOTE + PREPARATION_CHANGE,
        "the residual stays spendable in Orchard"
    );
    let residual_reserved = client
        .note_summaries(ShieldedPool::Orchard, false)
        .await
        .into_iter()
        .find(|note| note.value == RESIDUAL_NOTE)
        .map(|note| note.reserved);
    assert_eq!(
        residual_reserved,
        Some(false),
        "the residual note funds no transfer, so the schedule does not reserve it"
    );

    let scheduled = transfers_by_window(&client).await;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    assert!(
        scheduled
            .iter()
            .all(|transfer| transfer.window == Some(window)),
        "two transfers per window puts both in one window: {scheduled:?}"
    );
    broadcast_window(&net, &mut client, window).await;
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert!(
        status
            .transfers
            .iter()
            .all(|transfer| transfer.progress == TransferProgress::Confirmed),
        "every prepared transfer confirmed: {status:?}"
    );
    assert_eq!(
        status.phase,
        Some(MigrationPhase::Complete {
            residual: RESIDUAL_NOTE + PREPARATION_CHANGE
        }),
        "the migration completes with the disclosed residual: {status:?}"
    );
}

#[tokio::test]
async fn a_missed_window_reschedules_and_broadcast_missed_now_sends_it() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let missed = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");

    let boundary_after_window =
        u32::try_from(window + 1).expect("window boundaries fit a block height") * WINDOW;
    mine_to(&net, boundary_after_window).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks past the window scan");

    let status = client.migration_status().await.expect("the status reads");
    let rescheduled = &status.transfers[missed.0 as usize];
    assert_eq!(
        rescheduled.missed_windows, 1,
        "the closed window counts as missed: {status:?}"
    );
    assert!(
        rescheduled.window > Some(window),
        "reconciliation placed the transfer in a later window: {status:?}"
    );
    assert_eq!(
        rescheduled.progress,
        TransferProgress::Pending,
        "a missed transfer is still pending: {status:?}"
    );

    let report = client
        .broadcast_missed_now(Duration::ZERO)
        .await
        .expect("the disclosed send-now broadcasts");
    assert_all_sent(&report);
    mine_mempool_and_sync(&net, &mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[missed.0 as usize].progress,
        TransferProgress::Confirmed,
        "the caught-up transfer confirmed: {status:?}"
    );
}

#[tokio::test]
async fn a_lost_send_response_is_resubmitted_and_the_duplicate_counts_as_sent() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    advance_to_window(&net, &mut client, window).await;

    net.chain.write().await.lose_next_send_response = Some(LostSendDestination::Mempool);
    let lost = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a lost response is reported, not propagated");
    assert!(
        matches!(
            lost.outcomes.as_slice(),
            [outcome] if matches!(outcome.result, TransferBroadcastResult::Failed { .. })
        ),
        "the lost response is reported as a failed submission: {lost:?}"
    );

    let resubmitted = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("the still-open window resubmits");
    assert_all_sent(&resubmitted);
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Broadcast,
        "the endpoint's already-in-mempool answer proves the submission reached it"
    );

    mine_mempool_and_sync(&net, &mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transaction the endpoint already held was mined: {status:?}"
    );
}

#[tokio::test]
async fn a_signature_that_never_reached_the_mempool_is_discarded_and_rescheduled() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    advance_to_window(&net, &mut client, window).await;

    net.chain.write().await.reject_all_sends = true;
    let lost = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a refused submission is reported, not propagated");
    assert!(
        matches!(
            lost.outcomes.as_slice(),
            [outcome] if matches!(outcome.result, TransferBroadcastResult::Failed { .. })
        ),
        "the submission that never reached the mempool is reported failed: {lost:?}"
    );
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Signed,
        "the transfer keeps its signature while its window is open"
    );
    let abandoned = transfer_txid(&client, transfer)
        .await
        .expect("a signed transfer carries a txid");
    let status = client.migration_status().await.expect("the status reads");
    assert!(
        status
            .due_now
            .as_ref()
            .is_some_and(|batch| batch.transfer_ids.contains(&transfer)),
        "the transfer is still due in its own window: {status:?}"
    );

    let one_window_past =
        u32::try_from(window + 2).expect("window boundaries fit a block height") * WINDOW;
    mine_to(&net, one_window_past).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks past the following window scan");

    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Assigned,
        "reconciliation discarded a signature the chain never saw"
    );
    let status = client.migration_status().await.expect("the status reads");
    let rescheduled = &status.transfers[transfer.0 as usize];
    assert_eq!(
        rescheduled.missed_windows, 1,
        "the discarded signature counts one missed window: {status:?}"
    );
    let new_window = rescheduled
        .window
        .expect("the rescheduled transfer has a window");
    assert!(
        new_window > window,
        "the transfer moved to a later window: {status:?}"
    );

    net.chain.write().await.reject_all_sends = false;
    broadcast_window(&net, &mut client, new_window).await;
    let resigned = transfer_txid(&client, transfer)
        .await
        .expect("the rebuilt transfer carries a txid");
    assert_ne!(
        resigned, abandoned,
        "the rebuilt transfer is a different transaction"
    );
    mine_mempool_and_sync(&net, &mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the rebuilt transfer confirmed: {status:?}"
    );
}

#[tokio::test]
async fn a_reorg_demotes_a_confirmed_transfer_and_it_confirms_again() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;

    let mined_at = net.chain.read().await.tip();
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transfer confirmed before the reorg: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the single-transfer migration completed before the reorg: {status:?}"
    );

    let evicted = net.chain.write().await.reorg_to(mined_at);
    assert_eq!(evicted.len(), 1, "the reorg evicted the transfer's block");
    client
        .sync_and_await()
        .await
        .expect("the wallet rewinds to the fork point");
    mine_to(&net, mined_at + 1).await;
    client
        .sync_and_await()
        .await
        .expect("the wallet follows the new branch past the lost confirmation");

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Broadcast,
        "the reorged transfer is demoted to broadcast: {status:?}"
    );
    assert_eq!(
        status.phase,
        Some(MigrationPhase::Scheduled),
        "the completed migration reopens: {status:?}"
    );

    {
        let mut chain = net.chain.write().await;
        for raw_transaction in evicted {
            chain.enter_mempool(raw_transaction);
        }
    }
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the re-mined transfer confirmed again: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the migration completes again: {status:?}"
    );
}

#[tokio::test]
async fn a_committed_migration_reserves_its_notes_against_ordinary_sends() {
    let (_net, mut client) = funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE]).await;
    let recipient = external_address(PoolType::IRONWOOD);

    commit_scheduled_plan(&mut client).await;

    let refused = from_inputs::propose(&mut client, vec![(&recipient, 1_000_000, None)]).await;
    let ProposeSendError::ReservedForMigration { reserved } =
        refused.expect_err("the free notes cannot pay while every Orchard note is reserved")
    else {
        panic!("a send that needs the reserved notes fails with the typed refusal");
    };
    assert_eq!(
        reserved,
        ONE_MILLION_NOTE + TWO_MILLION_NOTE,
        "the refusal carries the reserved value"
    );

    let balance = balances(&client).await;
    assert_eq!(
        zats(balance.reserved_orchard_balance),
        ONE_MILLION_NOTE + TWO_MILLION_NOTE,
        "the reserved value is reported on its own"
    );
    assert_eq!(
        zats(balance.confirmed_orchard_balance),
        0,
        "the confirmed Orchard balance excludes the reserved notes"
    );
    let notes = client.note_summaries(ShieldedPool::Orchard, false).await;
    assert!(
        notes.into_iter().all(|note| note.reserved),
        "every pre-Ironwood Orchard note is flagged reserved"
    );

    client
        .cancel_migration()
        .await
        .expect("the migration cancels");

    from_inputs::propose(&mut client, vec![(&recipient, 1_000_000, None)])
        .await
        .expect("the released notes pay the same send");
    let balance = balances(&client).await;
    assert_eq!(
        zats(balance.reserved_orchard_balance),
        0,
        "a cancelled migration reserves nothing"
    );
    assert_eq!(
        zats(balance.confirmed_orchard_balance),
        ONE_MILLION_NOTE + TWO_MILLION_NOTE,
        "the whole Orchard balance is spendable again"
    );
}

#[tokio::test]
async fn a_released_transfer_frees_its_note_and_the_others_still_complete() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 2).await;

    let scheduled = transfers_by_window(&client).await;
    let released = scheduled
        .iter()
        .find(|transfer| transfer.denomination == 1_000_000)
        .expect("the smaller denomination is scheduled")
        .id;
    let kept = scheduled
        .iter()
        .find(|transfer| transfer.denomination == 2_000_000)
        .expect("the larger denomination is scheduled")
        .id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");

    client
        .release_transfer(released)
        .await
        .expect("a pending transfer releases");
    assert_eq!(
        transfer_state(&client, released).await,
        TransferState::Released,
        "the released transfer leaves the migration"
    );

    let recipient = external_address(PoolType::IRONWOOD);
    from_inputs::quick_send(&mut client, vec![(&recipient, 500_000, None)])
        .await
        .expect("the released note pays an ordinary send");
    mine_mempool_and_sync(&net, &mut client).await;

    broadcast_window(&net, &mut client, window).await;
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[kept.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transfer that stayed in the schedule confirmed: {status:?}"
    );
    assert_eq!(
        status.transfers[released.0 as usize].progress,
        TransferProgress::Released,
        "the released transfer stays released: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "a released transfer does not hold the migration open: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 2_000_000,
        "only the transfer that ran counts as migrated"
    );
}

#[tokio::test]
async fn an_immediate_migration_sweeps_the_orchard_pool_into_ironwood() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE, RESIDUAL_NOTE]).await;

    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .expect("planning is pure");
    let summary = client
        .migrate_immediately(AccountId::ZERO, &plan)
        .await
        .expect("the immediate migration builds and broadcasts");
    assert!(!summary.txids.is_empty(), "the sweep sent transactions");

    mine_mempool_and_sync(&net, &mut client).await;

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    for txid in &summary.txids {
        assert!(
            wallet
                .wallet_transactions
                .get(txid)
                .expect("the sweep transaction is in the wallet")
                .status()
                .is_confirmed(),
            "the sweep transaction {txid} was mined"
        );
    }
    drop(wallet);

    let balance = balances(&client).await;
    assert_eq!(
        zats(balance.confirmed_ironwood_balance),
        summary.migrated,
        "the Ironwood pool holds the migrated value"
    );
    assert_eq!(
        zats(balance.confirmed_orchard_balance),
        summary.residual,
        "only the residual is left in Orchard"
    );
    assert_eq!(
        summary.residual, RESIDUAL_NOTE,
        "the note below the sweep minimum is the residual"
    );
}

#[tokio::test]
async fn a_wallet_reloaded_mid_schedule_continues_the_schedule() {
    let (mut net, mut client) = funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let first = scheduled[0].id;
    let second = scheduled[1].id;
    let first_window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    let second_window = scheduled[1]
        .window
        .expect("a scheduled transfer has a window");

    broadcast_window(&net, &mut client, first_window).await;
    mine_mempool_and_sync(&net, &mut client).await;

    client.wallet().write().await.save_required = true;
    client.flush().await.expect("the wallet file writes");
    drop(client);

    let mut reloaded = net.client_from_file(0).await;
    reloaded.consent_to_clearnet_for_tests().await;
    reloaded.set_transmit_retry_interval(Duration::ZERO);
    reloaded
        .sync_and_await()
        .await
        .expect("the reloaded wallet syncs");

    let status = reloaded.migration_status().await.expect("the status reads");
    assert_eq!(
        status.phase,
        Some(MigrationPhase::Scheduled),
        "the reloaded wallet is still running its schedule: {status:?}"
    );
    assert_eq!(
        status.transfers[first.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transfer that confirmed before the reload is still confirmed: {status:?}"
    );
    assert_eq!(
        status.transfers[second.0 as usize].progress,
        TransferProgress::Pending,
        "the remaining transfer is still pending: {status:?}"
    );

    broadcast_window(&net, &mut reloaded, second_window).await;
    mine_mempool_and_sync(&net, &mut reloaded).await;
    reconcile_once_more(&mut reloaded).await;

    let status = reloaded.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[second.0 as usize].progress,
        TransferProgress::Confirmed,
        "the reloaded wallet finished the schedule: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the migration completes after the reload: {status:?}"
    );
    assert_eq!(
        status.value_migrated,
        1_000_000 + 2_000_000,
        "both transfers count as migrated"
    );
}

#[tokio::test]
async fn a_wallet_reopened_after_several_missed_windows_reschedules_and_completes() {
    let (mut net, mut client) = funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let submitted = scheduled[0].id;
    let untouched = scheduled[1].id;
    let first_window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    let last_window = scheduled[1]
        .window
        .expect("a scheduled transfer has a window");

    advance_to_window(&net, &mut client, first_window).await;
    net.chain.write().await.reject_all_sends = true;
    let refused = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a refused submission is reported, not propagated");
    assert!(
        matches!(
            refused.outcomes.as_slice(),
            [outcome] if matches!(outcome.result, TransferBroadcastResult::Failed { .. })
        ),
        "the submission never reaches the mempool: {refused:?}"
    );
    assert_eq!(
        transfer_state(&client, submitted).await,
        TransferState::Signed,
        "the refused transfer keeps its signature"
    );
    let abandoned = transfer_txid(&client, submitted)
        .await
        .expect("a signed transfer carries a txid");

    client.wallet().write().await.save_required = true;
    client.flush().await.expect("the wallet file writes");
    drop(client);

    const WINDOWS_AWAY: u64 = 4;
    let reopened_tip = u32::try_from(last_window + WINDOWS_AWAY)
        .expect("window boundaries fit a block height")
        * WINDOW
        + WINDOW / 2;
    mine_to(&net, reopened_tip).await;
    net.chain.write().await.reject_all_sends = false;

    let mut reopened = net.client_from_file(0).await;
    reopened.consent_to_clearnet_for_tests().await;
    reopened.set_transmit_retry_interval(Duration::ZERO);
    reopened
        .sync_and_await()
        .await
        .expect("the reopened wallet syncs");

    let current_window = u64::from(reopened_tip / WINDOW);
    let status = reopened.migration_status().await.expect("the status reads");
    assert_eq!(
        status.phase,
        Some(MigrationPhase::Scheduled),
        "the reopened wallet still runs its schedule: {status:?}"
    );
    for id in [submitted, untouched] {
        let transfer = &status.transfers[id.0 as usize];
        assert_eq!(
            transfer.progress,
            TransferProgress::Pending,
            "a missed transfer is still pending: {status:?}"
        );
        assert!(
            transfer.missed_windows >= 1,
            "the closed windows count as missed: {status:?}"
        );
        assert!(
            transfer.window > Some(current_window),
            "the transfer is placed in a window after the reopened tip: {status:?}"
        );
    }
    assert_ne!(
        status.transfers[submitted.0 as usize].window,
        status.transfers[untouched.0 as usize].window,
        "the reschedule keeps one transfer per window: {status:?}"
    );
    assert_eq!(
        transfer_state(&reopened, submitted).await,
        TransferState::Assigned,
        "the signature the chain never saw is discarded"
    );
    assert!(
        reopened
            .wallet()
            .read()
            .await
            .migration
            .as_ref()
            .expect("the migration is stored")
            .transfers()[submitted.0 as usize]
            .previous_txids
            .contains(&abandoned),
        "the discarded txid is archived"
    );
    assert_eq!(
        reserved_orchard_value(&reopened).await,
        ONE_MILLION_NOTE + TWO_MILLION_NOTE,
        "both funding notes stay reserved"
    );

    for transfer in transfers_by_window(&reopened).await {
        let window = transfer.window.expect("a pending transfer has a window");
        broadcast_window(&net, &mut reopened, window).await;
        mine_mempool_and_sync(&net, &mut reopened).await;
    }
    reconcile_once_more(&mut reopened).await;

    let resigned = transfer_txid(&reopened, submitted)
        .await
        .expect("the rebuilt transfer carries a txid");
    assert_ne!(
        resigned, abandoned,
        "the rebuilt transfer is a different transaction"
    );
    let status = reopened.migration_status().await.expect("the status reads");
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the migration completes after the reopen: {status:?}"
    );
    assert_eq!(
        status.value_migrated,
        1_000_000 + 2_000_000,
        "both transfers count as migrated"
    );
}

async fn transfer_of_denomination(client: &LightClient, denomination: u64) -> TransferId {
    transfers_by_window(client)
        .await
        .into_iter()
        .find(|transfer| transfer.denomination == denomination)
        .unwrap_or_else(|| panic!("a transfer of {denomination} zatoshis is scheduled"))
        .id
}

async fn transfer_status(client: &LightClient, id: TransferId) -> TransferStatus {
    client
        .migration_status()
        .await
        .expect("the status reads")
        .transfers[id.0 as usize]
        .clone()
}

async fn reserved_orchard_value(client: &LightClient) -> u64 {
    client.wallet().read().await.reserved_orchard_value()
}

async fn note_is_reserved(client: &LightClient, value: u64) -> Option<bool> {
    client
        .note_summaries(ShieldedPool::Orchard, false)
        .await
        .into_iter()
        .find(|note| note.value == value)
        .map(|note| note.reserved)
}

async fn assert_send_refused_for_migration(client: &mut LightClient, value: u64) {
    let recipient = external_address(PoolType::IRONWOOD);
    let refused = from_inputs::propose(client, vec![(&recipient, value, None)]).await;
    assert!(
        matches!(refused, Err(ProposeSendError::ReservedForMigration { .. })),
        "a send that needs the reserved note is refused: {refused:?}"
    );
}

async fn anchor_witness_root(client: &LightClient, id: TransferId) -> Option<[u8; 32]> {
    client
        .wallet()
        .read()
        .await
        .migration
        .as_ref()
        .expect("the migration is stored")
        .transfers()[id.0 as usize]
        .anchor_witness
        .as_ref()
        .map(|witness| witness.anchor)
}

async fn anchor_boundary(client: &LightClient, id: TransferId) -> u32 {
    let bucket = client
        .wallet()
        .read()
        .await
        .migration
        .as_ref()
        .expect("the migration is stored")
        .transfers()[id.0 as usize]
        .anchor_bucket
        .expect("a placed transfer carries an anchor bucket");
    u32::from(crate::wallet::migration::schedule::boundary_of(
        bucket, WINDOW,
    ))
}

async fn backdate_newest_wallet_block(client: &LightClient, seconds: u32) {
    let wallet = client.wallet();
    let mut wallet = wallet.write().await;
    let block = wallet
        .wallet_blocks
        .values_mut()
        .next_back()
        .expect("a synced wallet holds blocks");
    let backdated = block.time() - seconds;
    block.set_time_for_test(backdated);
}

#[tokio::test]
async fn a_second_proposal_does_not_re_enroll_a_released_transfer() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE, TWO_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let released = transfer_of_denomination(&client, 1_000_000).await;
    client
        .release_transfer(released)
        .await
        .expect("a pending transfer releases");

    let proposed = client
        .propose_schedule(1)
        .await
        .expect("a schedule with an unsigned transfer proposes again");
    assert_eq!(
        proposed
            .transfers
            .iter()
            .map(|transfer| transfer.denomination)
            .collect::<Vec<_>>(),
        vec![2_000_000],
        "the released note is not bound again: {proposed:?}"
    );
    client
        .commit_schedule(&proposed)
        .await
        .expect("the second schedule commits");

    let transfers = transfers_by_window(&client).await;
    assert_eq!(
        transfers.len(),
        2,
        "the released record stays beside the rebound transfer: {transfers:?}"
    );
    let released = transfer_of_denomination(&client, 1_000_000).await;
    let kept = transfer_of_denomination(&client, 2_000_000).await;
    assert_eq!(
        transfer_state(&client, released).await,
        TransferState::Released,
        "the released transfer is still released after the second commit"
    );
    assert_eq!(
        note_is_reserved(&client, ONE_MILLION_NOTE).await,
        Some(false),
        "the released transfer's funding note stays free"
    );

    let window = transfer_status(&client, kept)
        .await
        .window
        .expect("the rebound transfer has a window");
    broadcast_window(&net, &mut client, window).await;
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[kept.0 as usize].progress,
        TransferProgress::Confirmed,
        "the rebound transfer confirmed: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the migration completes without the released transfer: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 2_000_000,
        "only the rebound transfer counts as migrated"
    );
}

#[tokio::test]
async fn a_released_transfer_whose_transaction_mines_confirms_and_counts_as_migrated() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Broadcast,
        "the transfer is on the wire before it is released"
    );
    let broadcast_txid = transfer_txid(&client, transfer)
        .await
        .expect("a broadcast transfer carries a txid");

    client
        .release_transfer(transfer)
        .await
        .expect("a transfer on the wire releases");
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Released,
        "the released transfer leaves the migration"
    );
    assert_eq!(
        transfer_txid(&client, transfer).await,
        Some(broadcast_txid),
        "releasing a broadcast transfer keeps the txid of the transaction on the wire"
    );
    assert_eq!(
        reserved_orchard_value(&client).await,
        ONE_MILLION_NOTE,
        "the funding note stays reserved while its transaction may still mine"
    );
    assert_send_refused_for_migration(&mut client, 500_000).await;

    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the released transfer's transaction mined anyway: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 1_000_000,
        "the mined value counts as migrated"
    );
    assert_eq!(
        zats(balances(&client).await.confirmed_ironwood_balance),
        1_000_000,
        "the Ironwood pool holds the value the released transfer carried"
    );
}

#[tokio::test]
async fn cancel_migration_keeps_a_transfer_on_the_wire_until_it_mines() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;

    client
        .cancel_migration()
        .await
        .expect("a migration with a transfer on the wire cancels");
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Scheduled),
        "the record stays while its transaction may still mine"
    );
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Released,
        "cancelling releases the pending transfer"
    );
    assert_eq!(
        reserved_orchard_value(&client).await,
        ONE_MILLION_NOTE,
        "the funding note stays reserved while the transaction is on the wire"
    );
    assert_send_refused_for_migration(&mut client, 500_000).await;

    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the cancelled migration's transaction mined: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 1_000_000,
        "the mined value counts as migrated: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the record completes once the wire resolves: {status:?}"
    );
}

#[tokio::test]
async fn cancel_migration_frees_the_note_when_the_transaction_never_mines() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;

    client
        .cancel_migration()
        .await
        .expect("a migration with a transfer on the wire cancels");
    assert_eq!(
        reserved_orchard_value(&client).await,
        ONE_MILLION_NOTE,
        "the funding note stays reserved while the transaction is on the wire"
    );

    let one_window_past =
        u32::try_from(window + 2).expect("window boundaries fit a block height") * WINDOW;
    mine_to(&net, one_window_past).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks past the following window scan");
    reconcile_once_more(&mut client).await;

    assert_eq!(
        transfer_txid(&client, transfer).await,
        None,
        "the transaction the chain never saw is abandoned"
    );
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Released,
        "the abandoned transfer stays released: {status:?}"
    );
    assert_eq!(status.value_migrated, 0, "nothing migrated: {status:?}");
    assert_eq!(
        reserved_orchard_value(&client).await,
        0,
        "the abandoned wire reserves nothing"
    );
    assert_eq!(
        note_is_reserved(&client, ONE_MILLION_NOTE).await,
        Some(false),
        "the note is spendable again once the wire is abandoned"
    );
    let recipient = external_address(PoolType::IRONWOOD);
    from_inputs::propose(&mut client, vec![(&recipient, 500_000, None)])
        .await
        .expect("the freed note pays an ordinary send");
}

#[tokio::test]
async fn a_stale_chain_view_refuses_a_broadcast_until_a_fresh_block_scans() {
    const ONE_WINDOW_SECONDS: u32 = WINDOW * 75;

    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    advance_to_window(&net, &mut client, window).await;

    backdate_newest_wallet_block(&client, 2 * ONE_WINDOW_SECONDS).await;
    let refused = client.broadcast_due_transfers(Duration::ZERO).await;
    assert!(
        matches!(
            refused,
            Err(LightClientError::MigrationError(
                MigrationError::StaleChainView { .. }
            ))
        ),
        "a wallet that last saw the chain more than one window ago is refused: {refused:?}"
    );

    let tip = net.chain.read().await.tip();
    mine_to(&net, tip + 1).await;
    client
        .sync_and_await()
        .await
        .expect("the fresh block scans");
    let report = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a fresh chain view broadcasts");
    assert_all_sent(&report);

    mine_mempool_and_sync(&net, &mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transfer confirmed once the chain view was fresh: {status:?}"
    );
}

#[tokio::test]
async fn a_lost_response_from_the_download_queue_is_probed_and_counts_as_sent() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    advance_to_window(&net, &mut client, window).await;

    net.chain.write().await.lose_next_send_response = Some(LostSendDestination::DownloadQueue);
    let lost = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a lost response is reported, not propagated");
    assert!(
        matches!(
            lost.outcomes.as_slice(),
            [outcome] if matches!(outcome.result, TransferBroadcastResult::Failed { .. })
        ),
        "the response lost while queued for download is reported failed: {lost:?}"
    );

    net.chain.write().await.queued_rejections_before_promotion = 2;
    let resubmitted = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("the still-open window resubmits");
    assert_all_sent(&resubmitted);
    assert_eq!(
        net.chain.read().await.queued_rejections_before_promotion,
        0,
        "the queued-for-download rejections were probed through"
    );
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Broadcast,
        "the promoted duplicate proves the submission reached the endpoint"
    );

    mine_mempool_and_sync(&net, &mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the queued transaction was mined: {status:?}"
    );
}

#[tokio::test]
async fn broadcast_missed_now_rescues_a_refused_signature_whose_window_closed() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    advance_to_window(&net, &mut client, window).await;

    net.chain.write().await.reject_all_sends = true;
    let refused = client
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .expect("a refused submission is reported, not propagated");
    assert!(
        matches!(
            refused.outcomes.as_slice(),
            [outcome] if matches!(outcome.result, TransferBroadcastResult::Failed { .. })
        ),
        "the refused submission is reported failed: {refused:?}"
    );
    let refused_txid = transfer_txid(&client, transfer)
        .await
        .expect("the signed transfer carries a txid");

    let window_end =
        u32::try_from(window + 1).expect("window boundaries fit a block height") * WINDOW;
    mine_to(&net, window_end).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks past the window scan");
    assert_eq!(
        transfer_state(&client, transfer).await,
        TransferState::Signed,
        "the signature outlives its own window while the chain may still hold it"
    );

    net.chain.write().await.reject_all_sends = false;
    let report = client
        .broadcast_missed_now(Duration::ZERO)
        .await
        .expect("the disclosed send-now broadcasts");
    assert_all_sent(&report);

    let rescued_txid = transfer_txid(&client, transfer)
        .await
        .expect("the re-signed transfer carries a txid");
    assert_ne!(
        rescued_txid, refused_txid,
        "the rescue broadcast a freshly signed transaction"
    );
    assert_eq!(
        report.sent_txids(),
        vec![rescued_txid],
        "the new txid is the one the endpoint took: {report:?}"
    );
    let current_window = u64::from(net.chain.read().await.tip()) / u64::from(WINDOW);
    assert_eq!(
        transfer_status(&client, transfer).await.window,
        Some(current_window),
        "the rescued transfer moved into the window the chain is inside"
    );

    mine_mempool_and_sync(&net, &mut client).await;
    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the rescued transfer confirmed: {status:?}"
    );
}

#[tokio::test]
async fn a_polled_sync_confirms_a_mined_transfer_with_no_command_in_between() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;
    net.chain.write().await.mine_mempool();

    client.sync().await.expect("the sync task launches");
    loop {
        match client.poll_sync() {
            crate::data::PollReport::Ready(result) => {
                result.expect("the polled sync completes");
                break;
            }
            crate::data::PollReport::NotReady => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            crate::data::PollReport::NoHandle => panic!("the sync task is still owned"),
        }
    }

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the polled sync ran reconciliation itself: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 1_000_000,
        "the polled sync credited the migrated value: {status:?}"
    );
}

#[tokio::test]
async fn a_commit_after_a_reorg_of_a_complete_migration_keeps_the_confirmed_evidence() {
    let (net, mut client) = funded_client(&[ONE_MILLION_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;

    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    broadcast_window(&net, &mut client, window).await;

    let mined_at = net.chain.read().await.tip();
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;
    assert!(
        matches!(
            phase_of(&client).await,
            Some(MigrationPhase::Complete { .. })
        ),
        "the single-transfer migration completed before the reorg"
    );

    let evicted = net.chain.write().await.reorg_to(mined_at);
    assert_eq!(evicted.len(), 1, "the reorg evicted the transfer's block");

    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .expect("planning is pure");
    let committed = client.commit_migration(AccountId::ZERO, &plan).await;

    {
        let mut chain = net.chain.write().await;
        for raw_transaction in evicted {
            chain.enter_mempool(raw_transaction);
        }
    }
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert!(
        status
            .transfers
            .iter()
            .all(|transfer| transfer.progress != TransferProgress::Invalid),
        "the transfer's own re-mined transaction never invalidates it: {status:?}"
    );
    assert_eq!(
        zats(balances(&client).await.confirmed_ironwood_balance),
        1_000_000,
        "the Ironwood pool holds the value the transfer carried"
    );
    assert_eq!(
        status.value_migrated, 1_000_000,
        "the evidence of the confirmed transfer survives a commit taken while its confirmation \
         sat on an orphaned branch (commit_migration answered {committed:?}): {status:?}"
    );
    assert_eq!(
        status
            .transfers
            .get(transfer.0 as usize)
            .map(|status| status.progress),
        Some(TransferProgress::Confirmed),
        "the re-mined transfer is confirmed again: {status:?}"
    );
}

#[tokio::test]
async fn a_reorg_below_the_anchor_boundary_recaptures_the_witness_before_the_window() {
    const NEIGHBOUR_SEED: &str = zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;
    const NEIGHBOUR_NOTE: u64 = 2_000_000;

    let heights = migration_activation_heights();
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(heights)).await;
    let mut client = net.client(SEED).await;
    client.consent_to_clearnet_for_tests().await;
    client.set_transmit_retry_interval(Duration::ZERO);
    let mut neighbour = net.client(NEIGHBOUR_SEED).await;
    neighbour.consent_to_clearnet_for_tests().await;
    neighbour.set_transmit_retry_interval(Duration::ZERO);

    let address = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let neighbour_address =
        get_base_address(&neighbour, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let funding = pre_ironwood_funding_transaction(
        heights,
        vec![
            (address.as_str(), ONE_MILLION_NOTE, None),
            (neighbour_address.as_str(), NEIGHBOUR_NOTE, None),
        ],
    )
    .await;
    {
        let mut chain = net.chain.write().await;
        let to_funding = FUNDING_HEIGHT - chain.tip() - 1;
        chain.mine_empty_blocks(to_funding);
        chain.mine_block(vec![funding]);
        let past_activation = NU6_3_HEIGHT + 1 - chain.tip();
        chain.mine_empty_blocks(past_activation);
    }
    client
        .sync_and_await()
        .await
        .expect("the funding block scans");
    client
        .sync_and_await()
        .await
        .expect("the second pass completes the spend evidence");

    commit_scheduled_plan(&mut client).await;
    commit_schedule_of(&mut client, 1).await;
    let scheduled = transfers_by_window(&client).await;
    let transfer = scheduled[0].id;
    let window = scheduled[0]
        .window
        .expect("a scheduled transfer has a window");
    let boundary = anchor_boundary(&client, transfer).await;

    mine_to(&net, boundary - 3).await;
    neighbour
        .sync_and_await()
        .await
        .expect("the neighbour scans up to the fork point");
    let recipient = external_address(PoolType::IRONWOOD);
    from_inputs::quick_send(&mut neighbour, vec![(&recipient, 500_000, None)])
        .await
        .expect("the neighbour spends its own pre-Ironwood note");
    net.chain.write().await.mine_mempool();
    mine_to(&net, boundary).await;
    client
        .sync_and_await()
        .await
        .expect("the blocks up to the anchor boundary scan");
    let captured = anchor_witness_root(&client, transfer)
        .await
        .expect("the anchor witness is captured at the boundary");

    let evicted = net.chain.write().await.reorg_to(boundary - 3);
    assert!(
        !evicted.is_empty(),
        "the reorg evicted the neighbour's Orchard spend"
    );
    client
        .sync_and_await()
        .await
        .expect("the wallet rewinds below the neighbour's spend");
    mine_to(&net, boundary).await;
    client
        .sync_and_await()
        .await
        .expect("the wallet follows the new branch past the anchor boundary");

    let recaptured = anchor_witness_root(&client, transfer)
        .await
        .expect("the anchor witness is recaptured on the new branch");
    assert_ne!(
        recaptured, captured,
        "the branch without the neighbour's Orchard actions has another root at the boundary"
    );

    broadcast_window(&net, &mut client, window).await;
    mine_mempool_and_sync(&net, &mut client).await;
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert_eq!(
        status.transfers[transfer.0 as usize].progress,
        TransferProgress::Confirmed,
        "the transfer proved against the recaptured anchor and confirmed: {status:?}"
    );
    assert_eq!(
        status.value_migrated, 1_000_000,
        "the recaptured transfer's value migrated: {status:?}"
    );
}

#[tokio::test]
async fn a_note_set_that_changes_before_the_first_round_refuses_it_until_a_recommit() {
    let (mut net, mut client) = funded_client(&[UNPREPARED_NOTE, UNPREPARED_NOTE]).await;
    commit_scheduled_plan(&mut client).await;
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Committed),
        "two unprepared notes commit to Committed"
    );

    let mut twin = net.client(SEED).await;
    twin.consent_to_clearnet_for_tests().await;
    twin.set_transmit_retry_interval(Duration::ZERO);
    twin.sync_and_await()
        .await
        .expect("the twin of the same seed scans the funded chain");
    let recipient = external_address(PoolType::IRONWOOD);
    from_inputs::quick_send(&mut twin, vec![(&recipient, 2_500_000, None)])
        .await
        .expect("the twin spends one of the shared pre-Ironwood notes");
    mine_mempool_and_sync(&net, &mut client).await;
    client
        .sync_and_await()
        .await
        .expect("the spend evidence completes");

    let refused = client.broadcast_preparation_round().await;
    assert!(
        matches!(
            refused,
            Err(LightClientError::MigrationError(
                MigrationError::PlanMismatch
            ))
        ),
        "the first round refuses a plan the note set no longer matches: {refused:?}"
    );

    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .expect("planning is pure");
    client
        .recommit_migration(&plan)
        .await
        .expect("fresh consent to the current notes recommits");
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Committed),
        "the recommitted plan still needs a round"
    );

    let round = client
        .broadcast_preparation_round()
        .await
        .expect("the round broadcasts against the recommitted plan");
    assert!(!round.txids.is_empty(), "the round carries transactions");
    mine_mempool_and_sync(&net, &mut client).await;
    assert_eq!(
        phase_of(&client).await,
        Some(MigrationPhase::Prepared),
        "the confirmed round prepared the surviving note"
    );

    commit_schedule_of(&mut client, 1).await;
    let scheduled = transfers_by_window(&client).await;
    let denominations: u64 = scheduled.iter().map(|transfer| transfer.denomination).sum();
    assert!(
        denominations >= 2_000_000,
        "the recommitted schedule carries the prepared denominations: {scheduled:?}"
    );
    for transfer in &scheduled {
        let window = transfer.window.expect("a scheduled transfer has a window");
        broadcast_window(&net, &mut client, window).await;
        mine_mempool_and_sync(&net, &mut client).await;
    }
    reconcile_once_more(&mut client).await;

    let status = client.migration_status().await.expect("the status reads");
    assert!(
        status
            .transfers
            .iter()
            .all(|transfer| transfer.progress == TransferProgress::Confirmed),
        "every transfer of the recommitted migration confirmed: {status:?}"
    );
    assert!(
        matches!(status.phase, Some(MigrationPhase::Complete { .. })),
        "the recommitted migration completes: {status:?}"
    );
    assert_eq!(
        status.value_migrated, denominations,
        "the notes that survived the spend migrated: {status:?}"
    );
}
