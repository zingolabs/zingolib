use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::{
    KeyIdInterface as _, NoteInterface as _, OrchardNote, OutputId, OutputInterface as _,
    SyncState, WalletTransaction,
};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zip32::AccountId;

use crate::config::ChainType;
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::migrate::MigrationPlan;
use crate::mocks::transmission::{MOCK_DESTINATION, MOCK_SOCKS5_ADDR};
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_indexer::{MockChain, MockNet, pre_ironwood_funding_transaction};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::LightWallet;
use crate::wallet::migration::preparation::CANONICAL_TRANSFER_FEE;
use crate::wallet::migration::{
    BoundNote, BroadcastReceipt, BroadcastRoute, MigrationMode, MigrationParams, MigrationPhase,
    MigrationState, PlanCommitment, ReconcileReport, SigningStrategy, TransferId, TransferRecord,
    reconcile, schedule,
};

pub(super) const SEED: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;

pub(super) const NOTE_VALUE: u64 = 1_000_000;

pub(super) const FUNDING_NOTE: u64 = NOTE_VALUE + CANONICAL_TRANSFER_FEE;

pub(super) const RESIDUAL_NOTE: u64 = 500_000;

pub(super) const DUST_NOTE: u64 = 5_000;

pub(super) const UNPREPARED_NOTE: u64 = 1_000_000_000;

pub(super) const TIP: u32 = 360;

pub(super) const FAR_EXPIRY: u32 = 5_000;

pub(super) fn bound_note_of(wallet: &LightWallet, value: u64) -> BoundNote {
    wallet
        .wallet_transactions
        .values()
        .flat_map(OrchardNote::transaction_outputs)
        .find(|note| note.value() == value)
        .map(|note| BoundNote {
            output_id: note.output_id(),
            nullifier: note
                .nullifier()
                .expect("scanned notes carry nullifiers")
                .to_bytes(),
            commitment: [0; 32],
        })
        .expect("the wallet holds the fabricated note")
}

pub(super) fn wallet_with_migration_note(tip: u32) -> (LightWallet, BoundNote) {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(NOTE_VALUE)
        .tip(tip)
        .build();
    let bound_note = bound_note_of(&wallet, NOTE_VALUE);
    (wallet, bound_note)
}

pub(super) fn wallet_with_notes(tip: u32, count: usize) -> (LightWallet, Vec<BoundNote>) {
    let mut builder = SyntheticWalletBuilder::new(SEED);
    for _ in 0..count {
        builder = builder.orchard_note(NOTE_VALUE);
    }
    let wallet = builder.tip(tip).build();
    let notes: Vec<BoundNote> = wallet
        .wallet_transactions
        .values()
        .flat_map(OrchardNote::transaction_outputs)
        .filter(|note| note.value() == NOTE_VALUE)
        .map(|note| BoundNote {
            output_id: note.output_id(),
            nullifier: note
                .nullifier()
                .expect("scanned notes carry nullifiers")
                .to_bytes(),
            commitment: [0; 32],
        })
        .collect();
    assert_eq!(notes.len(), count, "the builder made every note");
    (wallet, notes)
}

pub(super) fn wallet_with_funding_notes(tip: u32, count: usize) -> LightWallet {
    let mut builder = SyntheticWalletBuilder::new(SEED);
    for _ in 0..count {
        builder = builder.orchard_note(FUNDING_NOTE);
    }
    builder.tip(tip).build()
}

pub(super) fn scheduled_state(
    params: MigrationParams,
    transfers: Vec<TransferRecord>,
) -> MigrationState {
    MigrationState {
        commitment: PlanCommitment {
            params_hash: params.params_hash(),
            plan_hash: [0; 32],
            committed_at: 0,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: MigrationMode::Scheduled,
        account: AccountId::ZERO,
        phase: MigrationPhase::Scheduled,
        transfers,
    }
}

pub(super) fn preparing_state(params: MigrationParams, phase: MigrationPhase) -> MigrationState {
    let mut state = scheduled_state(params, Vec::new());
    state.phase = phase;
    state
}

pub(super) fn current_bucket_of(wallet: &LightWallet, params: &MigrationParams) -> u64 {
    schedule::bucket_index(
        wallet
            .sync_state
            .last_known_chain_height()
            .expect("the synthetic wallet is fully synced"),
        params.bucket_modulus,
    )
}

pub(super) fn window_end_of(bucket: u64, params: &MigrationParams) -> BlockHeight {
    schedule::boundary_of(bucket + 1, params.bucket_modulus)
}

pub(super) fn assigned_transfer(id: u32, note: BoundNote, bucket: u64) -> TransferRecord {
    let mut transfer = TransferRecord::new(TransferId(id), NOTE_VALUE, note);
    transfer.assign(bucket).expect("fresh transfers assign");
    transfer
}

pub(super) fn signed_transfer(
    id: u32,
    note: BoundNote,
    bucket: u64,
    txid: TxId,
    expiry: u32,
    blob: Option<Vec<u8>>,
) -> TransferRecord {
    let mut transfer = assigned_transfer(id, note, bucket);
    transfer
        .mark_signed(txid, BlockHeight::from_u32(expiry), blob)
        .expect("assigned transfers sign");
    transfer
}

pub(super) fn broadcast_transfer(
    id: u32,
    note: BoundNote,
    bucket: u64,
    txid: TxId,
    expiry: u32,
) -> TransferRecord {
    let mut transfer = signed_transfer(id, note, bucket, txid, expiry, Some(vec![0xAB; 64]));
    transfer.record_attempt();
    transfer
        .mark_broadcast()
        .expect("signed transfers broadcast");
    transfer
}

pub(super) fn set_tip(wallet: &mut LightWallet, tip: u32) {
    wallet.sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
        BlockHeight::from_u32(1)..BlockHeight::from_u32(tip + 1),
        ScanPriority::Scanned,
    )]);
}

pub(super) fn set_tip_with_unscanned_gap(wallet: &mut LightWallet, scanned: u32, tip: u32) {
    wallet.sync_state = SyncState::new_for_test(vec![
        ScanRange::from_parts(
            BlockHeight::from_u32(1)..BlockHeight::from_u32(scanned + 1),
            ScanPriority::Scanned,
        ),
        ScanRange::from_parts(
            BlockHeight::from_u32(scanned + 1)..BlockHeight::from_u32(tip + 1),
            ScanPriority::Historic,
        ),
    ]);
}

pub(super) fn insert_calculated_transaction(wallet: &mut LightWallet, txid: TxId, height: u32) {
    wallet.wallet_transactions.insert(
        txid,
        WalletTransaction::new_for_test(
            txid,
            ConfirmationStatus::Calculated(BlockHeight::from_u32(height)),
        ),
    );
}

pub(super) fn transaction_status(wallet: &LightWallet, txid: &TxId) -> Option<ConfirmationStatus> {
    wallet
        .wallet_transactions
        .get(txid)
        .map(|transaction| transaction.status())
}

pub(super) fn creating_txid(wallet: &LightWallet, value: u64) -> TxId {
    wallet
        .wallet_transactions
        .iter()
        .find(|(_, transaction)| {
            OrchardNote::transaction_outputs(transaction)
                .iter()
                .any(|note| note.value() == value)
        })
        .map(|(txid, _)| *txid)
        .expect("the fabricated note has a creating transaction")
}

pub(super) fn unspent_v2_output_ids(wallet: &LightWallet) -> Vec<OutputId> {
    let mut ids: Vec<OutputId> = wallet
        .wallet_transactions
        .values()
        .flat_map(OrchardNote::transaction_outputs)
        .filter(|note| {
            note.key_id().account_id() == AccountId::ZERO
                && note.note().version() == orchard::note::NoteVersion::V2
                && note.spending_transaction().is_none()
        })
        .map(|note| note.output_id())
        .collect();
    ids.sort();
    ids
}

pub(super) fn sorted(mut ids: Vec<OutputId>) -> Vec<OutputId> {
    ids.sort();
    ids
}

pub(super) fn mark_note_spent_by(wallet: &mut LightWallet, bound: &BoundNote, spender: TxId) {
    let transaction = wallet
        .wallet_transactions
        .get_mut(&bound.output_id.txid())
        .expect("the bound note's transaction is in the wallet");
    let mut found = false;
    for note in transaction.orchard_notes_mut() {
        if note.output_id() == bound.output_id {
            note.set_spending_transaction(Some(spender));
            found = true;
        }
    }
    assert!(found, "the bound note was located");
}

pub(super) fn record_confirmed_transfer_spend(
    wallet: &mut LightWallet,
    bound: &BoundNote,
    spending_txid: TxId,
    height: u32,
) {
    mark_note_spent_by(wallet, bound, spending_txid);
    wallet.wallet_transactions.insert(
        spending_txid,
        WalletTransaction::new_for_test(
            spending_txid,
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(height)),
        ),
    );
}

pub(super) fn confirmed_transfer(bound: BoundNote, txid: TxId, height: u32) -> TransferRecord {
    let mut transfer = broadcast_transfer(0, bound, 1, txid, FAR_EXPIRY);
    transfer
        .mark_confirmed(BlockHeight::from_u32(height))
        .expect("broadcast transfers confirm");
    transfer
}

pub(super) fn wallet_with_one_confirmed_transfer(tip: u32, receipt: Option<u64>) -> LightWallet {
    const CONFIRMED_AT: u32 = 300;

    let mut builder = SyntheticWalletBuilder::new(SEED).orchard_note(FUNDING_NOTE);
    if let Some(value) = receipt {
        builder = builder.orchard_note(value);
    }
    let mut wallet = builder.tip(tip).build();

    let params = MigrationParams::provisional(wallet.chain_type());
    let bound = bound_note_of(&wallet, FUNDING_NOTE);
    let bucket = current_bucket_of(&wallet, &params);

    let transfer_txid = TxId::from_bytes([0xE1; 32]);
    let mut transfer = broadcast_transfer(0, bound, bucket, transfer_txid, FAR_EXPIRY);
    transfer
        .mark_confirmed(BlockHeight::from_u32(CONFIRMED_AT))
        .expect("broadcast transfers confirm");
    record_confirmed_transfer_spend(&mut wallet, &bound, transfer_txid, CONFIRMED_AT);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    wallet
}

pub(super) async fn client_with_a_scheduled_transfer(
    tip: u32,
    bucket: u64,
) -> (LightClient, ChainType) {
    let (mut wallet, bound_note) = wallet_with_migration_note(tip);
    let chain_type = wallet.chain_type();
    let params = MigrationParams::provisional(chain_type);
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, bucket)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.flush().await.expect("the baseline file writes");
    client.wallet().write().await.save_required = false;
    (client, chain_type)
}

pub(super) async fn scheduled_plan(client: &LightClient) -> MigrationPlan {
    client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .expect("planning is pure")
}

pub(super) async fn committed_client(wallet: LightWallet) -> LightClient {
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan just returned commits");
    client
}

pub(super) async fn scheduled_client(
    wallet: LightWallet,
    transfers_per_window: u32,
) -> LightClient {
    let mut client = committed_client(wallet).await;
    let proposed = client
        .propose_schedule(transfers_per_window)
        .await
        .expect("a prepared migration proposes");
    client
        .commit_schedule(&proposed)
        .await
        .expect("the proposal just drawn commits");
    client
}

pub(super) async fn migration_of(client: &LightClient) -> MigrationState {
    client
        .wallet()
        .read()
        .await
        .migration
        .clone()
        .expect("the migration exists")
}

pub(super) async fn phase_of(client: &LightClient) -> MigrationPhase {
    migration_of(client).await.phase
}

pub(super) async fn assessment_of(client: &LightClient) -> ReconcileReport {
    let wallet = client.wallet().read().await;
    let state = wallet.migration.as_ref().expect("the migration exists");
    reconcile(state, &*wallet)
}

pub(super) async fn on_disk(client: &LightClient, chain_type: ChainType) -> LightWallet {
    let bytes = std::fs::read(client.wallet_path()).expect("the wallet file exists");
    LightWallet::read(bytes.as_slice(), chain_type).expect("the file reads back")
}

pub(super) fn migration_error<T: std::fmt::Debug>(
    result: Result<T, LightClientError>,
) -> MigrationError {
    match result {
        Err(LightClientError::MigrationError(error)) => error,
        other => panic!("expected a migration error, got {other:?}"),
    }
}

pub(super) const MOCK_NU6_3_HEIGHT: u32 = 16;

pub(super) const MOCK_FUNDING_HEIGHT: u32 = 4;

pub(super) fn mock_chain_activation_heights() -> ActivationHeights {
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
        .set_nu6_3(Some(MOCK_NU6_3_HEIGHT))
        .set_nu7(None)
        .build()
}

pub(super) async fn funded_mock_client(note_values: &[u64]) -> (MockNet, LightClient) {
    let heights = mock_chain_activation_heights();
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(heights)).await;
    let mut client = net.client(SEED).await;
    client.consent_to_clearnet_for_tests().await;
    client.set_transmit_retry_interval(std::time::Duration::ZERO);

    let address = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let receivers = note_values
        .iter()
        .map(|value| (address.as_str(), *value, None))
        .collect();
    let funding = pre_ironwood_funding_transaction(heights, receivers).await;
    {
        let mut chain = net.chain.write().await;
        let to_funding = MOCK_FUNDING_HEIGHT - chain.tip() - 1;
        chain.mine_empty_blocks(to_funding);
        chain.mine_block(vec![funding]);
        let past_activation = MOCK_NU6_3_HEIGHT + 1 - chain.tip();
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

pub(super) async fn scheduled_mock_client(
    note_values: &[u64],
    transfers_per_window: u32,
) -> (MockNet, LightClient) {
    let (net, mut client) = funded_mock_client(note_values).await;
    let plan = scheduled_plan(&client).await;
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the scheduled plan commits");
    let proposed = client
        .propose_schedule(transfers_per_window)
        .await
        .expect("a prepared migration proposes a schedule");
    client
        .commit_schedule(&proposed)
        .await
        .expect("the proposed schedule commits");
    (net, client)
}

pub(super) async fn mine_to_and_sync(net: &MockNet, client: &mut LightClient, height: u32) {
    {
        let mut chain = net.chain.write().await;
        let tip = chain.tip();
        if tip < height {
            chain.mine_empty_blocks(height - tip);
        }
    }
    client
        .sync_and_await()
        .await
        .expect("the mined blocks scan");
}

pub(super) fn mixnet_receipt(txid: TxId) -> BroadcastReceipt {
    BroadcastReceipt {
        txid,
        route: BroadcastRoute::Mixnet {
            destination: MOCK_DESTINATION.to_string(),
            via_socks5: MOCK_SOCKS5_ADDR.to_string(),
        },
    }
}
