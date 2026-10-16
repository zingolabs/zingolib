use std::time::Duration;

use pepper_sync::wallet::{OutputId, OutputInterface, TransparentCoin};
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use super::{HOSPITAL_MUSEUM, untracked_history, zatoshis, zip317_fee};
use crate::check_client_balances;
use crate::lightclient::LightClient;
use crate::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
};
use crate::testutils::assertions::assert_null_data_send;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::MockNet;
use crate::wallet::output::SpendStatus;
use crate::wallet::summary::data::TransactionSummary;
use crate::wallet::transparent::OpReturnData;

const FAUCET_SEED: &str = zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;
const REWARD_ZATS: u64 = 625_000;
const REWARD: Zatoshis = Zatoshis::const_from_u64(REWARD_ZATS);
const SETUP_IRONWOOD_BLOCKS: u64 = 4;
const BLOCK_ONE_SAPLING_COINBASE: u64 = REWARD_ZATS;
const SAPLING_NOTE_STEP: u64 = 10_000;
const SAPLING_NOTES: u64 = 3;
const UNTOUCHED_SAPLING_NOTE: u64 = SAPLING_NOTE_STEP;
const LARGEST_SAPLING_NOTE: u64 = SAPLING_NOTES * SAPLING_NOTE_STEP;
// One transparent input plus the padded two-action ironwood pair.
const TRANSPARENT_SHIELD_LOGICAL_ACTIONS: u64 = 3;
const TRANSPARENT_FUNDING: u64 = 100_000;
const UNIFIED_PAYMENT: u64 = 50_000;
const OP_RETURN_FUNDING: u64 = 500_000;
const OP_RETURN_PAYMENT: Zatoshis = Zatoshis::const_from_u64(100_000);
const UNTRACKED_FUNDING: u64 = 100_000;

/// - Mines the mempool into a block whose shielded coinbase pays the faucet the reward plus `fee`.
async fn mine_mempool_to_faucet(net: &MockNet, faucet_ua: &str, fee: u64) {
    net.mine_mempool_rewarding_shielded(faucet_ua, zatoshis(REWARD_ZATS + fee))
        .await;
}

/// - Holds the wallet's read lock while it reads the transparent coins.
async fn sole_transparent_coin(client: &LightClient) -> (OutputId, SpendStatus) {
    let wallet = client.wallet().read().await;
    let coins = wallet.wallet_outputs::<TransparentCoin>();
    assert_eq!(coins.len(), 1);
    let coin = coins
        .first()
        .expect("the length assertion above leaves exactly one coin");
    (coin.output_id(), wallet.output_spend_status(*coin))
}

async fn send_orchard_back_and_forth(
    net: &MockNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    let faucet_to_recipient_amount = 20_000u64;
    let recipient_to_faucet_amount = 10_000u64;
    let fee = u64::from(MINIMUM_FEE);
    // check start state
    faucet
        .sync_and_await()
        .await
        .expect("the faucet syncs its coinbase rewards");
    let setup_tip = net.chain.read().await.tip();
    let wallet_fully_scanned_height = faucet
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height()
        .expect("the synced faucet records a fully scanned height");
    assert_eq!(
        wallet_fully_scanned_height,
        BlockHeight::from_u32(setup_tip)
    );
    let setup_reward = SETUP_IRONWOOD_BLOCKS * REWARD_ZATS;
    check_client_balances!(faucet, i: setup_reward o: 0 s: BLOCK_ONE_SAPLING_COINBASE t: 0);
    let faucet_ua = get_base_address(faucet, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let recipient_ua = get_base_address(recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    // post transfer to recipient, and verify
    from_inputs::quick_send(
        faucet,
        vec![(
            &recipient_ua,
            faucet_to_recipient_amount,
            Some("Orcharding"),
        )],
    )
    .await
    .expect("the faucet's send to the recipient builds and transmits");
    // The mined block credits the faucet (the miner) with a fresh reward
    // plus the send's fee; the send itself debits the amount and fee.
    // Aggregate balance is deterministic even though note selection is not.
    let ironwood_change = REWARD_ZATS - (faucet_to_recipient_amount + fee);
    mine_mempool_to_faucet(net, &faucet_ua, fee).await;
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the faucet's send");
    faucet
        .sync_and_await()
        .await
        .expect("the faucet syncs its send and the reward");
    let faucet_ironwood = setup_reward + ironwood_change + fee;
    check_client_balances!(faucet, i: faucet_ironwood o: 0 s: BLOCK_ONE_SAPLING_COINBASE t: 0);
    check_client_balances!(recipient, i: faucet_to_recipient_amount o: 0 s: 0 t: 0);

    // post half back to faucet, and verify
    from_inputs::quick_send(
        recipient,
        vec![(&faucet_ua, recipient_to_faucet_amount, Some("Sending back"))],
    )
    .await
    .expect("the recipient's send back builds and transmits");
    mine_mempool_to_faucet(net, &faucet_ua, fee).await;
    faucet
        .sync_and_await()
        .await
        .expect("the faucet syncs the returned value and the reward");
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its mined send");

    let faucet_final_ironwood = faucet_ironwood + recipient_to_faucet_amount + REWARD_ZATS + fee;
    let recipient_final_ironwood = faucet_to_recipient_amount - (fee + recipient_to_faucet_amount);
    check_client_balances!(faucet, i: faucet_final_ironwood o: 0 s: BLOCK_ONE_SAPLING_COINBASE t: 0);
    check_client_balances!(recipient, i: recipient_final_ironwood o: 0 s: 0 t: 0);
}

async fn multi_input_sapling_send_with_orchard_change_no_panic(
    net: &MockNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    net.chain.write().await.mine_empty_blocks(5);
    faucet
        .sync_and_await()
        .await
        .expect("the faucet syncs the empty blocks");

    let recipient_sapling =
        get_base_address(recipient, PoolType::Shielded(ShieldedPool::Sapling)).await;
    // Send three transfers in increasing 10_000 zat increments.
    let memos = ["1", "2", "3"];
    from_inputs::quick_send(
        faucet,
        (1..=SAPLING_NOTES)
            .map(|n| {
                (
                    recipient_sapling.as_str(),
                    n * SAPLING_NOTE_STEP,
                    Some(memos[(n - 1) as usize]),
                )
            })
            .collect(),
    )
    .await
    .expect("the three-note sapling funding builds and transmits");
    {
        let mut chain = net.chain.write().await;
        chain.mine_mempool();
        chain.mine_empty_blocks(5);
    }
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its three sapling notes");
    // Sending the largest single note the recipient holds back has to gather
    // two notes to cover the payment plus the fee.
    let faucet_ua = get_base_address(faucet, PoolType::Shielded(ShieldedPool::Orchard)).await;
    from_inputs::quick_send(
        recipient,
        vec![(
            &faucet_ua,
            LARGEST_SAPLING_NOTE,
            Some("Sending back, should have 2 inputs"),
        )],
    )
    .await
    .expect("the two-input sapling send builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its mined two-input send");
    // Post-state, observed and pinned: the 30_000 payment plus its
    // 20_000 ZIP-317 fee (two sapling spends, two orchard actions) is
    // covered exactly by the 20_000 and 30_000 notes, so the untouched
    // 10_000 sapling note is the whole remaining balance and the
    // orchard change is zero-value.
    check_client_balances!(recipient, i: 0 o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);
}

async fn sends_to_self_handle_balance_properly(
    net: &MockNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    let transfers_before_funding = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers")
        .iter()
        .count();
    let recipient_taddr = get_base_address(recipient, PoolType::Transparent).await;
    from_inputs::quick_send(faucet, vec![(&recipient_taddr, TRANSPARENT_FUNDING, None)])
        .await
        .expect("the transparent funding builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its transparent funding");
    let (preshield_coin, preshield_status) = sole_transparent_coin(recipient).await;
    assert!(preshield_status.is_unspent());

    recipient
        .quick_shield(AccountId::ZERO)
        .await
        .expect("the shield builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its mined shield");
    let (postshield_coin, postshield_status) = sole_transparent_coin(recipient).await;
    assert!(postshield_status.is_confirmed_spent());
    assert_eq!(preshield_coin, postshield_coin);

    // The shield sweeps the whole transparent funding into the Ironwood
    // pool (a V6 shield's change lands in the ironwood bundle), less the
    // ZIP-317 fee.
    let shielded_value = TRANSPARENT_FUNDING - zip317_fee(TRANSPARENT_SHIELD_LOGICAL_ACTIONS);
    check_client_balances!(recipient, i: shielded_value o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);

    let pre_rescan_summaries = recipient
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions");
    let pre_rescan_value_transfers = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers");
    // Exactly two new transfers: the transparent funding receipt and the
    // shield, the latter represented as a self-send of the Shield
    // kind carrying the swept value into the ironwood pool.
    assert_eq!(
        pre_rescan_value_transfers.iter().count(),
        transfers_before_funding + 2
    );
    assert!(pre_rescan_value_transfers.iter().any(|vt| {
        vt.kind == ValueTransferKind::Received
            && vt.value == TRANSPARENT_FUNDING
            && vt.pools_received == [PoolType::TRANSPARENT]
            && vt.pools_sent_from.is_empty()
    }));
    assert_eq!(
        pre_rescan_value_transfers
            .iter()
            .filter(|vt| {
                vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::Shield,
                    ))
                    && vt.value == shielded_value
                    && vt.transaction_fee == Some(zip317_fee(TRANSPARENT_SHIELD_LOGICAL_ACTIONS))
                    && vt.pools_sent_from == [PoolType::TRANSPARENT]
                    && vt.pools_received == [PoolType::IRONWOOD]
            })
            .count(),
        1
    );

    // Rescanning from scratch must reproduce the same balance and
    // identical records.
    recipient
        .rescan_and_await()
        .await
        .expect("a rescan from the birthday completes");
    check_client_balances!(recipient, i: shielded_value o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);
    assert_eq!(
        pre_rescan_summaries,
        recipient
            .transaction_summaries(false)
            .await
            .expect("a rescanned wallet summarizes its transactions")
    );
    assert_eq!(
        pre_rescan_value_transfers,
        recipient
            .value_transfers(true)
            .await
            .expect("a rescanned wallet lists its value transfers")
    );
}

async fn send_to_ua_saves_full_ua_in_wallet(
    net: &MockNet,
    faucet: &mut LightClient,
    recipient: &LightClient,
) {
    let recipient_unified_address =
        get_base_address(recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let txid = *from_inputs::quick_send(
        faucet,
        vec![(recipient_unified_address.as_str(), UNIFIED_PAYMENT, None)],
    )
    .await
    .expect("the funded faucet sends to a unified address")
    .first();
    net.chain.write().await.mine_mempool();
    faucet
        .sync_and_await()
        .await
        .expect("the faucet syncs its mined send");
    let saves_full_ua = |transaction: &TransactionSummary| {
        transaction.txid == txid
            && transaction
                .outgoing_ironwood_notes
                .iter()
                .chain(transaction.outgoing_sapling_notes.iter())
                .any(|note| {
                    note.recipient_unified_address == Some(recipient_unified_address.clone())
                })
    };
    let transactions = faucet
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions")
        .0;
    assert!(transactions.iter().any(saves_full_ua));
    faucet
        .rescan_and_await()
        .await
        .expect("a faucet rescan from the birthday completes");
    let rescanned_transactions = faucet
        .transaction_summaries(false)
        .await
        .expect("a rescanned wallet summarizes its transactions")
        .0;
    assert!(rescanned_transactions.iter().any(saves_full_ua));
    assert_eq!(transactions, rescanned_transactions);
}

async fn propose_and_send_with_op_return_confirms_on_chain(
    net: &MockNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    // A payload under the 80-byte limit.
    const PAYLOAD: &[u8] = b"zingolib op_return payload";
    let amount = OP_RETURN_PAYMENT;

    let recipient_ua = get_base_address(recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    from_inputs::quick_send(faucet, vec![(&recipient_ua, OP_RETURN_FUNDING, None)])
        .await
        .expect("the faucet's funding of the recipient builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its funding");

    let recipient_address = get_base_address(faucet, PoolType::Transparent).await;
    let data = OpReturnData::new(PAYLOAD.to_vec()).expect("the payload is under the limit");

    let proposal = recipient
        .propose_send_with_op_return(&recipient_address, amount, data, AccountId::ZERO)
        .await
        .expect("the OP_RETURN proposal builds");
    let op_return_fee = proposal.op_return_fee();
    let deshield_fee = proposal
        .deshield_fee()
        .expect("the deshield step carries a fee");
    assert_eq!(
        proposal.total_fee().expect("both fees sum"),
        (deshield_fee + op_return_fee).expect("both fees sum"),
        "total fee is the sum of both transaction fees"
    );
    assert_eq!(
        crate::data::proposal::total_payment_amount(proposal.deshield())
            .expect("the deshield pays one amount"),
        (amount + op_return_fee).expect("the amount plus the fee stays within range"),
        "the deshield pays the amount plus the OP_RETURN fee"
    );

    let txids = recipient
        .send_stored_proposal(false)
        .await
        .expect("both steps build and transmit");
    assert_eq!(txids.len(), 2, "a deshield and an OP_RETURN send are sent");
    let deshield_txid = txids[0];
    let op_return_txid = txids[1];

    {
        let mut chain = net.chain.write().await;
        chain.mine_mempool();
        chain.mine_empty_blocks(2);
    }
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs both mined steps");

    let wallet = recipient.wallet();
    let wallet = wallet.read().await;

    let deshield = wallet
        .wallet_transactions
        .get(&deshield_txid)
        .expect("deshield recorded");
    assert!(deshield.status().is_confirmed(), "deshield confirmed");

    let op_return_tx = wallet
        .wallet_transactions
        .get(&op_return_txid)
        .expect("OP_RETURN send recorded");
    assert!(
        op_return_tx.status().is_confirmed(),
        "OP_RETURN send confirmed"
    );

    let bundle = op_return_tx
        .transaction()
        .transparent_bundle()
        .expect("OP_RETURN send is a transparent transaction");

    assert_null_data_send(bundle, PAYLOAD);
    assert!(
        bundle.vout.iter().any(|out| out.value() == amount),
        "a recipient output pays the amount"
    );
}

/// - Mints a Sapling coinbase and four Ironwood coinbases to the faucet, then mines blocks on demand.
/// - Rescans both wallets from their birthdays and strips the recipient's Ironwood tracking last.
#[tokio::test]
async fn faucet_recipient_default_in_one_chain() {
    let mut net = MockNet::launch().await;
    let mut faucet = net.client(FAUCET_SEED, None).await;
    faucet.set_transmit_retry_interval(Duration::ZERO);
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    recipient.set_transmit_retry_interval(Duration::ZERO);
    let faucet_sapling = get_base_address(&faucet, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let faucet_ua = get_base_address(&faucet, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.mine_block_rewarding_shielded(&faucet_sapling, REWARD, vec![])
        .await;
    for _ in 0..SETUP_IRONWOOD_BLOCKS {
        net.mine_block_rewarding_shielded(&faucet_ua, REWARD, vec![])
            .await;
    }
    recipient
        .sync_and_await()
        .await
        .expect("the empty recipient syncs the setup chain");

    send_orchard_back_and_forth(&net, &mut faucet, &mut recipient).await;
    multi_input_sapling_send_with_orchard_change_no_panic(&net, &mut faucet, &mut recipient).await;
    sends_to_self_handle_balance_properly(&net, &mut faucet, &mut recipient).await;
    send_to_ua_saves_full_ua_in_wallet(&net, &mut faucet, &recipient).await;
    propose_and_send_with_op_return_confirms_on_chain(&net, &mut faucet, &mut recipient).await;

    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    from_inputs::quick_send(&mut faucet, vec![(&recipient_ua, UNTRACKED_FUNDING, None)])
        .await
        .expect("the faucet's final funding builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its final funding");
    untracked_history::recover_untracked_history(&net, &mut recipient, ShieldedPool::Ironwood)
        .await;
}
