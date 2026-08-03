#![forbid(unsafe_code)]
//! Mock-chain tests of the value-transfer classification, relocated from
//! zingolib's mock-chain test module when the derivation moved to this
//! crate: the real wallet pipeline driven against a fabricated in-process
//! chain, asserted through the editorial value-transfer views.
//!
//! Every port here is an OFFLINE TWIN: the live original stays in
//! libtonode-tests as the control group (user direction, 2026-07-08:
//! live versions are never removed).

use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;

use zingo_viewmodel::LightClientViewModelExt as _;
use zingo_viewmodel::{SelfSendValueTransfer, SentValueTransfer, ValueTransferKind};
use zingolib::check_client_balances;
use zingolib::testutils::lightclient::{from_inputs, get_base_address};
use zingolib::testutils::mock_indexer::{MockNet, faucet_funding_transaction};
use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;
use zingolib::wallet::keys::unified::ReceiverSelection;

/// An address belonging to no wallet on the mock net, so sends to it
/// are external.
fn external_address(pool: PoolType) -> String {
    let mut external_wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
    let selection = match pool {
        PoolType::ORCHARD | PoolType::IRONWOOD => ReceiverSelection::orchard_only(),
        PoolType::SAPLING => ReceiverSelection::sapling_only(),
        _ => unimplemented!("only shielded external destinations are needed here"),
    };
    let (_, unified_address) = external_wallet
        .generate_unified_address(selection, zip32::AccountId::ZERO)
        .unwrap();
    unified_address.encode(&external_wallet.chain_type())
}

/// Funds `client` with one faucet-built transaction mined into the next
/// mock block, followed by `extra_blocks` empty blocks.
async fn fund(net: &MockNet, receivers: Vec<(&str, u64, Option<&str>)>, extra_blocks: u32) {
    let funding = faucet_funding_transaction(receivers).await;
    let mut chain = net.chain.write().await;
    chain.mine_block(vec![funding]);
    chain.mine_empty_blocks(extra_blocks);
}

#[tokio::test]
async fn zero_value_receipts() {
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua = get_base_address(&recipient, PoolType::IRONWOOD).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    // The zero-value receipt, in its own block as on the live chain.
    fund(&net, vec![(&recipient_ua, 0, None)], 1).await;

    recipient.sync_and_await().await.unwrap();
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::IRONWOOD), 1_000, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    net.chain.write().await.mine_empty_blocks(1);
    recipient.sync_and_await().await.unwrap();

    // Identical to the live pin: the recipient holds the 100_000 funding
    // note less the 1_000 payment and its 10_000 ZIP-317 fee.
    check_client_balances!(recipient, i: 89_000 o: 0 s: 0 t: 0);

    let value_transfers = recipient.value_transfers(true).await.unwrap();
    assert!(
        value_transfers
            .iter()
            .any(|vt| vt.kind == ValueTransferKind::Received && vt.value == 100_000)
    );
    assert_eq!(
        value_transfers
            .iter()
            .filter(|vt| vt.kind == ValueTransferKind::Received
                && vt.value == 0
                && vt.pools_received == [PoolType::IRONWOOD])
            .count(),
        1
    );
    assert!(value_transfers.iter().any(|vt| {
        vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send)
            && vt.value == 1_000
            && vt.transaction_fee == Some(10_000)
    }));
}

/// A confirmed Orchard→Ironwood immediate migration transaction must surface in the
/// history as a `migration` value transfer, not `memo-to-self` and not
/// `basic`. Its self-received Ironwood output carries the canonical empty
/// memo (`MemoBytes::empty()`), so this pins the self-send classification
/// order in `value_transfers()`: the migration predicate must win over the
/// received-memo check regardless of how that memo decodes.
#[tokio::test]
async fn immediate_migration_is_a_migration_value_transfer() {
    use zip32::AccountId;

    use zingolib::testutils::synthetic_wallet::inject_confirmed_orchard_notes;

    const NOTE_VALUE: u64 = 1_000_000;
    const TIP: u32 = 41;

    // A real mock-net client, synced over an empty chain, handed one
    // spendable legacy-Orchard note whose nullifier is really derived, so
    // pepper-sync's spend detection marks it when the immediate migration spends it and
    // the summary sees the transaction as Orchard-funded.
    let mut net = MockNet::launch().await;
    net.chain.write().await.mine_empty_blocks(TIP);
    let mut client = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    client
        .sync_and_await()
        .await
        .expect("initial sync succeeds");
    {
        let wallet_lock = client.wallet().clone();
        let mut wallet = wallet_lock.write().await;
        inject_confirmed_orchard_notes(&mut wallet, 1, NOTE_VALUE, TIP);
    }

    let summary = client
        .migrate_immediately(AccountId::ZERO)
        .await
        .expect("the immediate migration builds and broadcasts");
    assert_eq!(
        summary.txids.len(),
        1,
        "one note migrates in one transaction"
    );

    net.chain.write().await.mine_mempool();
    client.sync_and_await().await.unwrap();

    let value_transfers = client.value_transfers(false).await.unwrap();
    let kinds: Vec<_> = value_transfers.iter().map(|vt| vt.kind).collect();
    assert!(
        kinds.contains(&ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
            SelfSendValueTransfer::Migration,
        ))),
        "the immediate migration transaction must classify as a migration value transfer; got {kinds:?}",
    );
}

/// An Orchard-funded self-send that lands in the Ironwood pool AND carries a
/// received memo must still classify as `migration`, not `memo-to-self`: the
/// migration predicate wins the self-send classification regardless of
/// memos, and the memo itself stays on the value transfer. This is the
/// ordering pin for `value_transfers()`: before the reorder the memo check
/// fired first and relabeled the migration `memo-to-self`.
#[tokio::test]
async fn migration_with_memo_is_still_a_migration_value_transfer() {
    use zingolib::testutils::synthetic_wallet::inject_confirmed_orchard_notes;

    const NOTE_VALUE: u64 = 1_000_000;
    const TIP: u32 = 41;
    const MEMO: &str = "moving my own funds";

    let mut net = MockNet::launch().await;
    net.chain.write().await.mine_empty_blocks(TIP);
    let mut client = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    client
        .sync_and_await()
        .await
        .expect("initial sync succeeds");
    {
        let wallet_lock = client.wallet().clone();
        let mut wallet = wallet_lock.write().await;
        inject_confirmed_orchard_notes(&mut wallet, 1, NOTE_VALUE, TIP);
    }

    // A send to the wallet's own orchard receiver lands in the Ironwood pool
    // post-NU6.3, funded from the legacy Orchard note: an Orchard→Ironwood
    // self-send carrying a real memo. Asserted on the pending (transmitted)
    // record, the state the history shows right after broadcast, and the
    // same classification path as a confirmed transaction. (Mining it would
    // conflict the injected note's fabricated orchard tree leaf with the
    // send's real orchard commitments at the same positions.)
    let own_ua = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    from_inputs::quick_send(&mut client, vec![(&own_ua, 50_000, Some(MEMO))])
        .await
        .unwrap();

    let value_transfers = client.value_transfers(false).await.unwrap();
    let migration = value_transfers
        .iter()
        .find(|vt| {
            vt.kind
                == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                    SelfSendValueTransfer::Migration,
                ))
        })
        .unwrap_or_else(|| {
            panic!(
                "the memo-carrying Orchard→Ironwood self-send must classify as a \
                 migration value transfer; got {:?}",
                value_transfers.iter().map(|vt| vt.kind).collect::<Vec<_>>(),
            )
        });
    assert!(
        migration.memos.iter().any(|memo| memo == MEMO),
        "the migration value transfer must keep its memo; got {:?}",
        migration.memos,
    );
}
