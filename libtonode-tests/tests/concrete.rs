#![forbid(unsafe_code)]
use json::JsonValue;

use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;

use pepper_sync::wallet::{OutputId, TransparentCoin};
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::wallet::summary::data::TransactionSummary;
use zingolib::{check_client_balances, get_base_address_macro};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
use zingolib_testutils::setup_metrics::MeteredNet;

#[path = "support/untracked_history.rs"]
mod untracked_history;

use pepper_sync::wallet::OutputInterface;
use zingolib::testutils::lightclient::from_inputs;

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

use zingolib::lightclient::LightClient;
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
};
use zingolib::wallet::output::SpendStatus;

async fn sends_to_self_handle_balance_properly(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    let transparent_funding = 100_000;
    let transfers_before_funding = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers")
        .iter()
        .count();
    from_inputs::quick_send(
        faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            transparent_funding,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(local_net, recipient, 1)
        .await
        .unwrap();
    let (preshield_coin, preshield_status) = sole_transparent_coin(recipient).await;
    assert!(preshield_status.is_unspent());

    recipient
        .quick_shield(zip32::AccountId::ZERO)
        .await
        .unwrap();
    increase_height_and_wait_for_client(local_net, recipient, 1)
        .await
        .unwrap();
    let (postshield_coin, postshield_status) = sole_transparent_coin(recipient).await;
    assert!(postshield_status.is_confirmed_spent());
    assert_eq!(preshield_coin, postshield_coin);

    // The shield sweeps the whole transparent funding into the Ironwood
    // pool (a V6 shield's change lands in the ironwood bundle, ADR 0009),
    // less the 15_000 ZIP-317 fee (one transparent input plus the padded
    // two-action ironwood pair).
    let shielded_value = transparent_funding - 15_000;
    check_client_balances!(recipient, i: shielded_value o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);

    let pre_rescan_summaries = recipient.transaction_summaries(false).await.unwrap();
    let pre_rescan_value_transfers = recipient.value_transfers(true).await.unwrap();
    // Exactly two new transfers: the transparent funding receipt and the
    // shield, the latter represented as a self-send of the Shield
    // kind carrying the swept value into the orchard pool.
    assert_eq!(
        pre_rescan_value_transfers.iter().count(),
        transfers_before_funding + 2
    );
    assert!(pre_rescan_value_transfers.iter().any(|vt| {
        vt.kind == ValueTransferKind::Received
            && vt.value == transparent_funding
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
                    && vt.transaction_fee == Some(15_000)
                    && vt.pools_sent_from == [PoolType::TRANSPARENT]
                    && vt.pools_received == [PoolType::IRONWOOD]
            })
            .count(),
        1
    );

    // Rescanning from scratch must reproduce the same balance and
    // identical records.
    recipient.rescan_and_await().await.unwrap();
    check_client_balances!(recipient, i: shielded_value o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);
    assert_eq!(
        pre_rescan_summaries,
        recipient.transaction_summaries(false).await.unwrap()
    );
    assert_eq!(
        pre_rescan_value_transfers,
        recipient.value_transfers(true).await.unwrap()
    );
}
async fn send_to_ua_saves_full_ua_in_wallet(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &LightClient,
) {
    let recipient_unified_address = get_base_address_macro!(recipient, "unified");
    let sent_value = 50_000;
    let txid = *from_inputs::quick_send(
        faucet,
        vec![(recipient_unified_address.as_str(), sent_value, None)],
    )
    .await
    .expect("the funded faucet sends to a unified address")
    .first();
    increase_height_and_wait_for_client(local_net, faucet, 1)
        .await
        .unwrap();
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
    let transactions = faucet.transaction_summaries(false).await.unwrap().0;
    assert!(transactions.iter().any(saves_full_ua));
    faucet.rescan_and_await().await.unwrap();
    let rescanned_transactions = faucet.transaction_summaries(false).await.unwrap().0;
    assert!(rescanned_transactions.iter().any(saves_full_ua));
    assert_eq!(
        transactions,
        rescanned_transactions,
        "Pre-Rescan: {}\n\n\nPost-Rescan: {}\n\n\n",
        json::stringify_pretty(transactions.clone(), 4),
        json::stringify_pretty(rescanned_transactions.clone(), 4)
    );
}

async fn send_orchard_back_and_forth(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    let faucet_to_recipient_amount = 20_000u64;
    let recipient_to_faucet_amount = 10_000u64;
    // check start state
    faucet.sync_and_await().await.unwrap();
    let wallet_fully_scanned_height = faucet
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height()
        .unwrap();
    assert_eq!(
        wallet_fully_scanned_height,
        scenarios::FUNDED_FAUCET_SETUP_HEIGHT.into()
    );
    let setup_reward = scenarios::funded_faucet_ironwood_balance();
    check_client_balances!(faucet, i:setup_reward o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);

    // post transfer to recipient, and verify
    from_inputs::quick_send(
        faucet,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            faucet_to_recipient_amount,
            Some("Orcharding"),
        )],
    )
    .await
    .unwrap();
    // The mined block credits the faucet (the miner) with a fresh
    // post-stream coinbase reward plus the send's fee; the send itself
    // debits the amount and fee. Aggregate balance is deterministic even
    // though note selection is not.
    let ironwood_change =
        scenarios::POST_STREAM_BLOCK_REWARD - (faucet_to_recipient_amount + u64::from(MINIMUM_FEE));
    increase_height_and_wait_for_client(local_net, recipient, 1)
        .await
        .unwrap();
    faucet.sync_and_await().await.unwrap();
    let faucet_ironwood = setup_reward + ironwood_change + u64::from(MINIMUM_FEE);

    tracing::info!(
        "{}",
        JsonValue::from(faucet.value_transfers(true).await.unwrap()).pretty(4)
    );
    tracing::info!(
        "{}",
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );

    check_client_balances!(faucet, i:faucet_ironwood o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);
    check_client_balances!(recipient, i: faucet_to_recipient_amount o: 0 s: 0 t: 0);

    // post half back to faucet, and verify
    from_inputs::quick_send(
        recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            recipient_to_faucet_amount,
            Some("Sending back"),
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(local_net, faucet, 1)
        .await
        .unwrap();
    recipient.sync_and_await().await.unwrap();

    let faucet_final_ironwood = faucet_ironwood
        + recipient_to_faucet_amount
        + scenarios::POST_STREAM_BLOCK_REWARD
        + u64::from(MINIMUM_FEE);
    let recipient_final_ironwood =
        faucet_to_recipient_amount - (u64::from(MINIMUM_FEE) + recipient_to_faucet_amount);
    check_client_balances!(
        faucet,
        i: faucet_final_ironwood o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
    check_client_balances!(recipient, i: recipient_final_ironwood o: 0 s: 0 t: 0);
}

/// The final send must gather more than one of the recipient's three
/// sapling notes (10_000, 20_000, 30_000) to cover 30_000 plus the fee,
/// making this the suite's only live broadcast of a multi-input sapling
/// spend with cross-pool (orchard) change. The unwraps assert proposal,
/// proving, and the validator's acceptance of the bundle. The closing
/// balance check pins the spend's post-state (gap 1a of the audit's
/// remediation plan). Note-selection ordering itself is asserted
/// offline by `note_selection_covers_target_with_minimal_change` in
/// `zingolib::lightclient::propose`.
const SAPLING_NOTE_STEP: u64 = 10_000;
const UNTOUCHED_SAPLING_NOTE: u64 = SAPLING_NOTE_STEP;
async fn multi_input_sapling_send_with_orchard_change_no_panic(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    increase_height_and_wait_for_client(local_net, faucet, 5)
        .await
        .unwrap();

    let client_2_saplingaddress = get_base_address_macro!(recipient, "sapling");
    // Send three transfers in increasing 10_000 zat increments
    // These are sent from the coinbase funded client which will
    // subsequently receive funding via it's orchard-packed UA.
    let memos = ["1", "2", "3"];
    from_inputs::quick_send(
        faucet,
        (1..=3)
            .map(|n| {
                (
                    client_2_saplingaddress.as_str(),
                    n * SAPLING_NOTE_STEP,
                    Some(memos[(n - 1) as usize]),
                )
            })
            .collect(),
    )
    .await
    .unwrap();

    increase_height_and_wait_for_client(local_net, recipient, 5)
        .await
        .unwrap();
    // We know that the largest single note that 2 received from 1 was 30_000, for 2 to send
    // 30_000 back to 1 it will have to collect funds from two notes to pay the full 30_000
    // plus the transaction fee.
    from_inputs::quick_send(
        recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            30_000,
            Some("Sending back, should have 2 inputs"),
        )],
    )
    .await
    .unwrap();

    increase_height_and_wait_for_client(local_net, recipient, 1)
        .await
        .unwrap();
    // Post-state, observed and pinned: the 30_000 payment plus its
    // 20_000 ZIP-317 fee (two sapling spends, two orchard actions) is
    // covered exactly by the 20_000 and 30_000 notes, so the untouched
    // 10_000 sapling note is the whole remaining balance and the
    // orchard change is zero-value.
    check_client_balances!(recipient, i: 0 o: 0 s: UNTOUCHED_SAPLING_NOTE t: 0);
}

// FIXME: add unified address discovery to pepper sync and add a test here

mod basic_transactions {

    // FIXME: zingo2 rewrite action / inputs / outputs counting using new interface
    // #[tokio::test]
    // async fn standard_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid2 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "sapling").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid3 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&faucet, txid1.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid2.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid2.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid2.as_str()).await
    //     );

    //     let tx_actions_txid2 =
    //         tx_actions(&faucet, Some(&recipient), txid2.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid2);

    //     let calculated_fee_txid2 =
    //         total_tx_value(&faucet, txid2.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid2);

    //     let expected_fee_txid2 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid2.transparent_tx_actions
    //                 + tx_actions_txid2.sapling_tx_actions
    //                 + tx_actions_txid2.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid2);

    //     assert_eq!(calculated_fee_txid2, expected_fee_txid2 as u64);

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid3.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid3.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid3.as_str()).await
    //     );

    //     let tx_actions_txid3 =
    //         tx_actions(&faucet, Some(&recipient), txid3.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid3);

    //     let calculated_fee_txid3 =
    //         total_tx_value(&faucet, txid3.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid3);

    //     let expected_fee_txid3 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid3.transparent_tx_actions
    //                 + tx_actions_txid3.sapling_tx_actions
    //                 + tx_actions_txid3.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid3);

    //     assert_eq!(calculated_fee_txid3, expected_fee_txid3 as u64);

    //     let txid4 = lightclient::from_inputs::quick_send(
    //         &recipient,
    //         vec![(
    //             get_base_address_macro!(faucet, "transparent").as_str(),
    //             55_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&recipient, txid4.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&faucet, txid4.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&recipient, txid4.as_str()).await
    //     );

    //     let tx_actions_txid4 =
    //         tx_actions(&recipient, Some(&faucet), txid4.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid4);

    //     let calculated_fee_txid4 =
    //         total_tx_value(&recipient, txid4.as_str()).await - 55_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid4);

    //     let expected_fee_txid4 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid4.transparent_tx_actions
    //                 + tx_actions_txid4.sapling_tx_actions
    //                 + tx_actions_txid4.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid4);

    //     assert_eq!(calculated_fee_txid4, expected_fee_txid4 as u64);
    // }

    // #[tokio::test]
    // async fn dust_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             0,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&faucet, txid1.as_str()).await;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);
    // }

    // #[tokio::test]
    // async fn shield_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     let txid1 = recipient.quick_shield().await.unwrap().first().to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&recipient, None, txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&recipient, txid1.as_str()).await;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();
    // }
}

/// `propose_send_with_op_return` reports the fee of both transactions.
/// `send_stored_proposal` broadcasts a deshield and a transparent-only
/// transaction from a shielded balance. After mining, both confirm. The
/// second transaction pays the recipient, carries the payload in an
/// OP_RETURN output, and has no change. The deshield pays exactly the
/// amount plus the OP_RETURN fee.
async fn propose_and_send_with_op_return_confirms_on_chain(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
) {
    use zingolib::wallet::transparent::OpReturnData;

    /// A payload under the 80-byte limit.
    const PAYLOAD: &[u8] = b"zingolib op_return payload";
    let amount = Zatoshis::const_from_u64(100_000);

    scenarios::send_and_bump(
        local_net,
        faucet,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            500_000,
            None,
        )],
    )
    .await;
    recipient.sync_and_await().await.unwrap();

    let recipient_address = get_base_address_macro!(faucet, "transparent");
    let data = OpReturnData::new(PAYLOAD.to_vec()).unwrap();

    let proposal = recipient
        .propose_send_with_op_return(&recipient_address, amount, data, zip32::AccountId::ZERO)
        .await
        .unwrap();
    let op_return_fee = proposal.op_return_fee();
    let deshield_fee = proposal.deshield_fee().unwrap();
    assert_eq!(
        proposal.total_fee().unwrap(),
        (deshield_fee + op_return_fee).unwrap(),
        "total fee is the sum of both transaction fees"
    );
    assert_eq!(
        zingolib::data::proposal::total_payment_amount(proposal.deshield()).unwrap(),
        (amount + op_return_fee).unwrap(),
        "the deshield pays the amount plus the OP_RETURN fee"
    );

    let txids = recipient.send_stored_proposal(false).await.unwrap();
    assert_eq!(txids.len(), 2, "a deshield and an OP_RETURN send are sent");
    let deshield_txid = txids[0];
    let op_return_txid = txids[1];

    increase_height_and_wait_for_client(local_net, recipient, 3)
        .await
        .unwrap();

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

    assert_eq!(bundle.vin.len(), 1, "spends the single deshield output");

    assert_eq!(
        bundle.vout.len(),
        2,
        "recipient + OP_RETURN, no change output"
    );

    let op_return_out = bundle
        .vout
        .iter()
        .find(|out| out.value() == Zatoshis::ZERO)
        .expect("a zero-value OP_RETURN output");
    let script = op_return_out.script_pubkey().0.0.clone();
    assert_eq!(script[0], 0x6a, "null-data script starts with OP_RETURN");
    assert!(
        script.windows(PAYLOAD.len()).any(|w| w == PAYLOAD),
        "the OP_RETURN carries the payload verbatim"
    );

    let recipient_out = bundle
        .vout
        .iter()
        .find(|out| out.value() == amount)
        .expect("a recipient output paying the amount");
    assert_ne!(
        recipient_out.value(),
        Zatoshis::ZERO,
        "the recipient output is the non-null-data output"
    );
}

/// - Launches a validator and an indexer, and mines blocks on their chain.
/// - Writes the faucet and recipient wallet files, and rescans both from scratch.
/// - Strips the recipient wallet's Ironwood tracking in the final phase.
#[tokio::test]
async fn faucet_recipient_default_in_one_chain() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    send_orchard_back_and_forth(&local_net, &mut faucet, &mut recipient).await;
    multi_input_sapling_send_with_orchard_change_no_panic(&local_net, &mut faucet, &mut recipient)
        .await;
    sends_to_self_handle_balance_properly(&local_net, &mut faucet, &mut recipient).await;
    send_to_ua_saves_full_ua_in_wallet(&local_net, &mut faucet, &recipient).await;
    propose_and_send_with_op_return_confirms_on_chain(&local_net, &mut faucet, &mut recipient)
        .await;
    untracked_history::assert_notes_in_untracked_history_are_recovered(
        &local_net,
        &mut faucet,
        &mut recipient,
        ShieldedPool::Ironwood,
    )
    .await;
}

mod testnet_test {
    use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
    use zingolib::perspective::sync_progress;
    use zingolib::{
        config::{ChainType, ClientConfig, WalletConfig},
        lightclient::LightClient,
        testutils::{default_test_wallet_settings, tempfile::TempDir},
    };

    /// The testnet indexer these wallet-load tests pin explicitly.
    const TESTNET_INDEXER: &str = "https://testnet.zec.rocks:443";

    #[ignore = "testnet cannot be run offline"]
    #[tokio::test]
    async fn reload_wallet_after_short_sync() {
        zingolib::ensure_default_crypto_provider();

        const NUM_TESTS: u8 = 20;
        let mut test_count = 0;

        while test_count < NUM_TESTS {
            let wallet_dir = TempDir::new().unwrap();
            let config = ClientConfig::builder()
                .set_chain_type(ChainType::Testnet)
                .set_indexer_uri((TESTNET_INDEXER).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: 1.try_into().unwrap(),
                    birthday: 2_000_000,
                    wallet_settings: default_test_wallet_settings(),
                })
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build()
                .unwrap();

            let mut lightclient = LightClient::new(config, true).await.unwrap();
            lightclient.save_task().await;
            lightclient.sync().await.unwrap();
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            while lightclient.latest_sync_status().is_none_or(|status| {
                sync_progress::SyncProgress::from(status).percentage_total_outputs_scanned < 1.0
            }) {
                interval.tick().await;
            }
            lightclient.stop_sync().unwrap();
            lightclient.await_sync().await.unwrap();
            lightclient.shutdown_save_task().await.unwrap();

            // will fail if there were any reload errors due to bad file write code i.e. no flushing or file syncing
            let config = ClientConfig::builder()
                .set_chain_type(ChainType::Testnet)
                .set_indexer_uri((TESTNET_INDEXER).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::Read)
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build()
                .unwrap();
            LightClient::new(config, true).await.unwrap();

            test_count += 1;
        }
    }
}
