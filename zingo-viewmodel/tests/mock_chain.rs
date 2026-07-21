#![forbid(unsafe_code)]
//! Mock-chain tests of the editorial derivation, relocated from
//! zingolib's `lightclient::mock_chain_tests` when the value-transfer
//! derivation moved to this crate. The live libtonode originals remain
//! the control group, as for every mock-chain twin.

use zcash_protocol::PoolType;

use zingo_viewmodel::{LightClientViewModelExt as _, SentValueTransfer, ValueTransferKind};
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

/// Funds a client with one faucet-built transaction mined into the next
/// mock block, followed by `extra_blocks` empty blocks.
async fn fund(net: &MockNet, receivers: Vec<(&str, u64, Option<&str>)>, extra_blocks: u32) {
    let funding = faucet_funding_transaction(receivers).await;
    let mut chain = net.chain.write().await;
    chain.mine_block(vec![funding]);
    chain.mine_empty_blocks(extra_blocks);
}

/// Mock-chain twin of libtonode `slow::zero_value_receipts` (live
/// original kept as the control): a zero-value receipt must surface as
/// exactly one Received{0, Orchard} value transfer and must not perturb
/// spendable arithmetic across a subsequent send.
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
