use pepper_sync::wallet::{IronwoodNote, OutputInterface, SaplingNote};
use zcash_primitives::transaction::TxId;
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_status::confirmation_status::ConfirmationStatus;

use super::{external_address, funded_client};
use crate::lightclient::LightClient;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::mock_indexer::MockNet;
use crate::testutils::synthetic_wallet;
use crate::wallet::output::SpendStatus;

const FUNDED: u64 = 1_000_000;
const SMALL: u64 = 2_000;
const BIG: u64 = 100_000;
const STEADY_BLOCKS: u32 = 10;
const STABILITY_BLOCKS: u32 = 9;

async fn assert_ironwood_split(client: &LightClient, total: u64, confirmed: u64, unconfirmed: u64) {
    let balance = client
        .account_balance(zip32::AccountId::ZERO)
        .await
        .expect("a synced client reports a balance");
    let zatoshis = |value: Option<zcash_protocol::value::Zatoshis>| {
        value
            .expect("an Ironwood-capable wallet reports every Ironwood balance")
            .into_u64()
    };
    assert_eq!(zatoshis(balance.total_ironwood_balance), total);
    assert_eq!(zatoshis(balance.confirmed_ironwood_balance), confirmed);
    assert_eq!(zatoshis(balance.unconfirmed_ironwood_balance), unconfirmed);
}

async fn assert_ironwood_note_statuses(
    client: &LightClient,
    expected: &[(u64, SpendStatus, Option<bool>)],
) {
    let wallet = client.wallet().read().await;
    assert_eq!(wallet.wallet_outputs::<SaplingNote>().len(), 0);
    let ironwood_notes = wallet.wallet_outputs::<IronwoodNote>();
    assert_eq!(ironwood_notes.len(), expected.len());
    for (value, status, confirmed) in expected {
        let note = (*ironwood_notes
            .iter()
            .find(|&&note| note.value() == *value)
            .unwrap_or_else(|| panic!("no ironwood note of value {value}")))
        .clone();
        assert_eq!(wallet.output_spend_status(&note), *status, "note {value}");
        if let Some(want_confirmed) = confirmed {
            assert_eq!(
                wallet.output_transaction(&note).status().is_confirmed(),
                *want_confirmed,
                "note {value} transaction confirmation"
            );
        }
    }
}

/// - Runs one sync session on `client`.
async fn sync_into_mempool(client: &mut LightClient, txid: TxId) {
    client
        .sync_and_await()
        .await
        .expect("the mock chain serves every block and mempool entry the wallet requests");
    let status = client
        .wallet()
        .read()
        .await
        .wallet_transactions
        .get(&txid)
        .map(|transaction| transaction.status());
    assert!(
        matches!(status, Some(ConfirmationStatus::Mempool(_))),
        "the mock mempool is synchronous, so one sync session records transaction {txid} in \
         the mempool, but its status is {status:?}"
    );
}

#[tokio::test]
async fn mempool_spend_balance_and_note_status_accounting() {
    let mut net = MockNet::launch().await;
    let mut recipient = funded_client(&mut net, &[(PoolType::ORCHARD, FUNDED)], 1).await;

    // Steady state, and its stability across an empty ten-block mine.
    assert_ironwood_split(&recipient, FUNDED, FUNDED, 0).await;
    net.chain.write().await.mine_empty_blocks(STEADY_BLOCKS);
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the empty blocks");
    assert_ironwood_split(&recipient, FUNDED, FUNDED, 0).await;

    // Cycle one: a small send whose change sits near the dust line.
    let small_txids = from_inputs::quick_send(
        &mut recipient,
        vec![(
            &external_address(PoolType::ORCHARD),
            SMALL,
            Some("Outgoing Memo"),
        )],
    )
    .await
    .expect("the small send builds and transmits");
    sync_into_mempool(&mut recipient, *small_txids.first()).await;
    let small_fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::ORCHARD, true);
    let after_small = FUNDED - (SMALL + small_fee);
    assert_ironwood_split(&recipient, after_small, 0, after_small).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (
                FUNDED,
                SpendStatus::MempoolSpent(*small_txids.first()),
                None,
            ),
            (after_small, SpendStatus::Unspent, Some(false)),
        ],
    )
    .await;

    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined small send");
    assert_ironwood_split(&recipient, after_small, after_small, 0).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (FUNDED, SpendStatus::Spent(*small_txids.first()), None),
            (after_small, SpendStatus::Unspent, Some(true)),
        ],
    )
    .await;

    // Cycle two: a larger cross-pool send, spending the change note.
    let big_txids = from_inputs::quick_send(
        &mut recipient,
        vec![(&synthetic_wallet::external_sapling_address(), BIG, None)],
    )
    .await
    .expect("the cross-pool send builds and transmits");
    sync_into_mempool(&mut recipient, *big_txids.first()).await;
    let big_fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::SAPLING, true);
    let after_big = after_small - (BIG + big_fee);
    assert_ironwood_split(&recipient, after_big, 0, after_big).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (FUNDED, SpendStatus::Spent(*small_txids.first()), None),
            (
                after_small,
                SpendStatus::MempoolSpent(*big_txids.first()),
                None,
            ),
            (after_big, SpendStatus::Unspent, Some(false)),
        ],
    )
    .await;

    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined cross-pool send");
    assert_ironwood_split(&recipient, after_big, after_big, 0).await;
    let settled = [
        (FUNDED, SpendStatus::Spent(*small_txids.first()), None),
        (after_small, SpendStatus::Spent(*big_txids.first()), None),
        (after_big, SpendStatus::Unspent, Some(true)),
    ];
    assert_ironwood_note_statuses(&recipient, &settled).await;

    // Stability window: nine further blocks change nothing at
    // either granularity.
    net.chain.write().await.mine_empty_blocks(STABILITY_BLOCKS);
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the stability window");
    assert_ironwood_split(&recipient, after_big, after_big, 0).await;
    assert_ironwood_note_statuses(&recipient, &settled).await;
}
