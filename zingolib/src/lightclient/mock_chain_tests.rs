//! Ports of chain-bound libtonode tests onto the stateful mock indexer
//! ([`crate::testutils::mock_indexer`]): the real wallet pipeline
//! (`GrpcIndexer`, pepper-sync scanning, record building, spend
//! bookkeeping) driven against a fabricated in-process chain, with no
//! zebrad or zainod.
//!
//! Every port here is an OFFLINE TWIN: the live original stays in
//! libtonode-tests as the control group (user direction, 2026-07-08:
//! live versions are never removed. They eventually move to a gated
//! "pre-migration" mod once side-by-side equivalence is documented).

use pepper_sync::wallet::IronwoodNote;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;

use crate::check_client_balances;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::keys::unified::ReceiverSelection;

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

/// The mock-net proof: a funding block scans to a confirmed balance,
/// a real quick_send round-trips through the mock's mempool into the
/// next block, and the post-confirmation balance carries the exact
/// ZIP-317 arithmetic.
#[tokio::test]
async fn funded_send_confirms_on_the_mock_chain() {
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;

    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);

    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // 100_000 funding minus the 20_000 payment and its 10_000 one-orchard-
    // spend, two-logical-action ZIP-317 fee.
    check_client_balances!(recipient, i: 70_000 o: 0 s: 0 t: 0);
}

/// Mock-chain twin of libtonode `slow::list_value_transfers_check_fees`
/// (live original kept as the control): a two-output cross-pool send to
/// the wallet's own transparent and sapling addresses costs the exact
/// composite ZIP-317 fee: 5_000 for the transparent output, 10_000 for
/// the orchard bundle view carrying the ironwood spend, 10_000 for the
/// sapling output pair, 10_000 for the ironwood change pair (ADR
/// 0007). Every pool balance lands where the arithmetic says. The
/// self-receipts arrive through real scanning of the mock blocks.
#[tokio::test]
async fn list_value_transfers_check_fees() {
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let recipient_taddr = get_base_address(&recipient, PoolType::Transparent).await;
    let recipient_sapling =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Sapling)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);

    from_inputs::quick_send(
        &mut recipient,
        vec![
            (recipient_taddr.as_str(), 30_000, None),
            (recipient_sapling.as_str(), 30_000, None),
        ],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // 100_000 − 30_000 − 30_000 − 25_000 fee = 15_000 orchard change.
    check_client_balances!(recipient, i: 15_000 o: 0 s: 30_000 t: 30_000);
}

/// Mock-chain twin of libtonode
/// `slow::self_send_to_t_displays_as_one_transaction` (live original
/// kept as the control): mixed self-sends to the wallet's own
/// transparent, sapling, and orchard addresses (plus an incoming
/// mixed send mined in the same block) must each surface as ONE
/// transaction, so every transaction-summary txid is unique.
#[tokio::test]
async fn self_send_to_t_displays_as_one_transaction() {
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let recipient_taddr = get_base_address(&recipient, PoolType::Transparent).await;
    let recipient_zaddr =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Sapling)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 80_000, None)], 0).await;
    recipient.sync_and_await().await.unwrap();

    let sent_to_taddr_value = 5_000;
    let sent_to_zaddr_value = 11_000;
    let sent_to_self_orchard_value = 1_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // The recipient's own mixed self-send and an incoming mixed send,
    // mined into the same block as on the live chain.
    from_inputs::quick_send(
        &mut recipient,
        vec![
            (recipient_taddr.as_str(), sent_to_taddr_value, None),
            (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo")),
            (
                recipient_ua.as_str(),
                sent_to_self_orchard_value,
                Some("bar"),
            ),
        ],
    )
    .await
    .unwrap();
    let incoming = faucet_funding_transaction(vec![
        (recipient_taddr.as_str(), sent_to_taddr_value, None),
        (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo2")),
        (
            recipient_ua.as_str(),
            sent_to_self_orchard_value,
            Some("bar2"),
        ),
    ])
    .await;
    {
        let mut chain = net.chain.write().await;
        chain.submit_transaction(incoming);
        chain.mine_mempool();
    }
    recipient.sync_and_await().await.unwrap();

    let txids: Vec<_> = recipient
        .transaction_summaries(false)
        .await
        .unwrap()
        .iter()
        .map(|summary| summary.txid)
        .collect();
    let unique: std::collections::HashSet<_> = txids.iter().collect();
    assert_eq!(
        unique.len(),
        txids.len(),
        "every self-send surfaces as exactly one transaction"
    );
}

/// Mock-chain twin of libtonode
/// `slow::send_to_transparent_and_sapling_maintain_balance` (live
/// original kept as the control): full transaction-summary pinning
/// across funding waves, cross-pool sends, and the
/// Transmitted-to-Confirmed transition of an unmined send.
///
/// One deliberate divergence from the live literals: the second
/// funding wave's fee is Some(10_000) here, not the live Some(20_000).
/// That fee belongs to the FAUCET's economics (its live note pool is
/// fragmented by earlier waves. The mock faucet is fresh each wave) and
/// says nothing about the recipient behavior this test protects.
#[tokio::test]
async fn send_to_transparent_and_sapling_maintain_balance() {
    use zcash_protocol::consensus::BlockHeight;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_test_vectors::TEST_TXID;

    use crate::testutils::{
        assert_transaction_summary_equality, assert_transaction_summary_exists,
    };
    use crate::utils::conversion::txid_from_hex_encoded_str;
    use crate::wallet::output::SpendStatus;
    use crate::wallet::summary::data::Scope as SummaryScope;
    use crate::wallet::summary::data::{
        BasicNoteSummary, OutgoingNoteSummary, SendType, TransactionKind, TransactionSummary,
    };

    let recipient_initial_funds = 100_000_000;
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua = get_base_address(&recipient, PoolType::IRONWOOD).await;
    // The external destinations: the abandon-art wallet's sapling UA and
    // first taddr, the same derivations the live faucet answers with.
    let external_sapling = external_address(PoolType::SAPLING);
    let external_taddr = {
        let external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        external_wallet
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone()
    };

    // Funding at height 2, as in the live layout (one empty launch block).
    net.chain.write().await.mine_empty_blocks(1);
    fund(
        &net,
        vec![(&recipient_ua, recipient_initial_funds, None)],
        0,
    )
    .await;
    recipient.sync_and_await().await.unwrap();

    let placeholder_txid = txid_from_hex_encoded_str(TEST_TXID).unwrap();
    let summary_orchard_receipt = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(2)),
        blockheight: BlockHeight::from_u32(2),
        kind: TransactionKind::Received,
        value: recipient_initial_funds,
        fee: Some(10_000),
        zec_price: None,
        pools_sent_from: vec![],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            recipient_initial_funds,
            SpendStatus::Spent(placeholder_txid),
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
    };

    // Send to external sapling, mined at height 3.
    let first_send_to_sapling = 20_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_sapling, first_send_to_sapling, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();
    let summary_external_sapling = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(3)),
        blockheight: BlockHeight::from_u32(3),
        kind: TransactionKind::Sent(SendType::Send),
        value: first_send_to_sapling,
        fee: Some(20_000),
        zec_price: None,
        pools_sent_from: vec![PoolType::IRONWOOD],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            99_960_000,
            SpendStatus::TransmittedSpent(placeholder_txid),
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![OutgoingNoteSummary {
            output_index: 0,
            value: first_send_to_sapling,
            memo: None,
            recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
            recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
            account_id: zip32::AccountId::ZERO,
            scope: SummaryScope::from(zip32::Scope::External),
        }],
        outgoing_transparent_coins: vec![],
    };

    // Send to external transparent, left in the mempool: Transmitted,
    // targeting height 4.
    let first_send_to_transparent = 20_000;
    let summary_external_transparent = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Transmitted(BlockHeight::from_u32(4)),
        blockheight: BlockHeight::from_u32(4),
        kind: TransactionKind::Sent(SendType::Send),
        value: first_send_to_transparent,
        fee: Some(15_000),
        zec_price: None,
        pools_sent_from: vec![PoolType::IRONWOOD],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            99_925_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
    };
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_taddr, first_send_to_transparent, None)],
    )
    .await
    .unwrap();

    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[0],
        &summary_orchard_receipt,
    );
    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[1],
        &summary_external_sapling,
    );
    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[2],
        &summary_external_transparent,
    );

    // Mid-flight balances: everything sits in the unmined send's change.
    let expected_funds = recipient_initial_funds
        - first_send_to_sapling
        - 20_000
        - first_send_to_transparent
        - 15_000;
    {
        let recipient_wallet = recipient.wallet();
        let recipient_wallet = recipient_wallet.read().await;
        assert_eq!(
            recipient_wallet
                .unconfirmed_balance::<IronwoodNote>(zip32::AccountId::ZERO)
                .unwrap(),
            expected_funds.try_into().unwrap()
        );
        assert_eq!(
            recipient_wallet
                .confirmed_balance::<IronwoodNote>(zip32::AccountId::ZERO)
                .unwrap(),
            0.try_into().unwrap()
        );
    }

    // The pending transparent send confirms at height 4.
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // Second funding wave, with a memo, at height 5.
    let recipient_second_funding = 1_000_000;
    fund(
        &net,
        vec![(
            &recipient_ua,
            recipient_second_funding,
            Some("Second wave incoming"),
        )],
        0,
    )
    .await;
    recipient.sync_and_await().await.unwrap();
    let summary_orchard_receipt_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(5)),
        blockheight: BlockHeight::from_u32(5),
        kind: TransactionKind::Received,
        value: recipient_second_funding,
        // The documented divergence: the mock faucet is fresh, so its
        // funding send is the plain two-action 10_000, not the live
        // fragmented-faucet 20_000.
        fee: Some(10_000),
        zec_price: None,
        pools_sent_from: vec![],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            recipient_second_funding,
            SpendStatus::Spent(placeholder_txid),
            0,
            Some("Second wave incoming".to_string()),
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
    };

    // Second wave of external sends: transparent and sapling mined into
    // the same block, height 6.
    let second_send_to_transparent = 20_000;
    let second_send_to_sapling = 20_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_taddr, second_send_to_transparent, None)],
    )
    .await
    .unwrap();
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_sapling, second_send_to_sapling, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();
    let summary_external_transparent_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(6)),
        blockheight: BlockHeight::from_u32(6),
        kind: TransactionKind::Sent(SendType::Send),
        value: second_send_to_transparent,
        fee: Some(15_000),
        zec_price: None,
        pools_sent_from: vec![PoolType::IRONWOOD],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            965_000,
            SpendStatus::Spent(placeholder_txid),
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
    };
    let summary_external_sapling_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(6)),
        blockheight: BlockHeight::from_u32(6),
        kind: TransactionKind::Sent(SendType::Send),
        value: second_send_to_sapling,
        fee: Some(20_000),
        zec_price: None,
        pools_sent_from: vec![PoolType::IRONWOOD],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            99_885_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![OutgoingNoteSummary {
            output_index: 0,
            value: second_send_to_sapling,
            memo: None,
            recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
            recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
            account_id: zip32::AccountId::ZERO,
            scope: SummaryScope::from(zip32::Scope::External),
        }],
        outgoing_transparent_coins: vec![],
    };

    // Third external transparent, mined at height 7.
    let external_transparent_3 = 20_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_taddr, external_transparent_3, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();
    let summary_external_transparent_3 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(7)),
        blockheight: BlockHeight::from_u32(7),
        kind: TransactionKind::Sent(SendType::Send),
        value: external_transparent_3,
        fee: Some(15_000),
        zec_price: None,
        pools_sent_from: vec![PoolType::IRONWOOD],
        ironwood_notes: vec![BasicNoteSummary::from_parts(
            930_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        orchard_notes: vec![],
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
    };

    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[3],
        &summary_orchard_receipt_2,
    );
    // Two sends share height 6, so summary order within the block is
    // not pinned; assert existence as the live test does.
    assert_transaction_summary_exists(&recipient, &summary_external_transparent_2).await;
    assert_transaction_summary_exists(&recipient, &summary_external_sapling_2).await;
    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[6],
        &summary_external_transparent_3,
    );

    let second_wave_expected_funds = expected_funds + recipient_second_funding
        - second_send_to_sapling
        - second_send_to_transparent
        - external_transparent_3
        - 50_000;
    assert_eq!(
        recipient
            .wallet()
            .read()
            .await
            .confirmed_balance::<IronwoodNote>(zip32::AccountId::ZERO)
            .unwrap(),
        second_wave_expected_funds.try_into().unwrap(),
    );
}

/// Mock-chain twin of libtonode `slow::from_t_z_o_tz_to_zo_tzo_to_orchard`
/// (live original kept as the control): the full pool-promotion ledger,
/// every funding source and self-send combination, two shields, exact
/// per-step balances, and the cumulative confirmed-fee total, driven
/// through real scanning of the mock chain.
///
/// The `darkside_test` hazard of zingolabs/zingolib#2447 is gone: that
/// subtractive feature compiled out the transparent-address discovery
/// this test's funding depends on whenever feature unification enabled
/// it in multi-package builds, and the feature and its gates are now
/// deleted. The test stays ignored for an unrelated reason: its ledger
/// predates V6 and every step needs re-deriving per ADR 0009.
#[ignore = "The ledger's fees and amounts predate V6. Re-derive every step per ADR 0009 \
            before un-ignoring (step 10 under-drains, stranding 10_000 in sapling, because \
            V6's two-bundle fees lead the planner to leave the sapling note unspent)"]
#[tokio::test]
async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
    use crate::lightclient::error::{LightClientError, SendError};
    use crate::testutils::lightclient::get_fees_paid_by_client;
    use crate::wallet::error::ProposeSendError;
    use zcash_protocol::value::Zatoshis;

    let mut net = MockNet::launch().await;
    let mut client = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let pmc_unified = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let pmc_taddr = get_base_address(&client, PoolType::Transparent).await;
    let pmc_sapling = get_base_address(&client, PoolType::Shielded(ShieldedPool::Sapling)).await;

    net.chain.write().await.mine_empty_blocks(1);

    macro_rules! bump_and_check {
        (o: $o:tt i: $i:tt s: $s:tt t: $t:tt) => {
            net.chain.write().await.mine_mempool();
            client.sync_and_await().await.unwrap();
            check_client_balances!(client, i: $i o:$o s:$s t:$t);
        };
    }

    let mut total_expected_fee = 0;
    // 1 receive 50_000 transparent. Expanded rather than using
    // bump_and_check so a failure surfaces the mock's taddr-request
    // ledger and the wallet's record count (nextest shows this stderr
    // only when the test fails).
    fund(&net, vec![(&pmc_taddr, 50_000, None)], 0).await;
    net.chain.write().await.mine_mempool();
    client.sync_and_await().await.unwrap();
    {
        let chain = net.chain.read().await;
        eprintln!("step-1 diagnostics: mock tip {}", chain.tip());
        eprintln!("taddr requests served: {:#?}", chain.taddr_request_log());
        let wallet = client.wallet();
        let wallet = wallet.read().await;
        eprintln!("wallet transactions: {}", wallet.wallet_transactions.len());
    }
    check_client_balances!(client, i: 0 o: 0 s: 0 t: 50_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 2 shield 50_000 transparent to orchard: 15_000 (1 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 0 i: 35_000 s: 0 t: 0);
    total_expected_fee += 15_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 3 receive 50_000 sapling
    fund(&net, vec![(&pmc_sapling, 50_000, None)], 0).await;
    bump_and_check!(o: 0 i: 35_000 s: 50_000 t: 0);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 4 migrate sapling to orchard: 20_000 (2 sapling, 2 orchard)
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 65_000 s: 0 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 5 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 55_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 55_000 s: 0 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 6 orchard to own transparent and sapling: 25_000
    from_inputs::quick_send(
        &mut client,
        vec![(&pmc_taddr, 10_000, None), (&pmc_sapling, 10_000, None)],
    )
    .await
    .unwrap();
    bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 10_000);
    total_expected_fee += 25_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 7 receive 500_000 transparent
    fund(&net, vec![(&pmc_taddr, 500_000, None)], 0).await;
    bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 510_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 8 shield both coins: 20_000 (2 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 0 i: 500_000 s: 10_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 9 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 490_000 s: 10_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 10 orchard + sapling demoted to own transparent: 30_000
    from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 470_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 0 s: 0 t: 470_000);
    total_expected_fee += 30_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 10b transparent-to-transparent is refused: transparent funds are
    // not send-spendable.
    match from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 10_000, None)]).await {
        Err(LightClientError::SendError(SendError::ProposeSendError(ProposeSendError::Plan(
            crate::wallet::spend::plan::PlanError::InsufficientFunds {
                available,
                required,
            },
        )))) => {
            assert_eq!(available, Zatoshis::from_u64(0).unwrap());
            assert_eq!(required, Zatoshis::from_u64(20_000).unwrap());
        }
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
    bump_and_check!(o: 0 i: 0 s: 0 t: 470_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 11 transparent-to-sapling likewise refused.
    match from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 50_000, None)]).await {
        Err(LightClientError::SendError(SendError::ProposeSendError(ProposeSendError::Plan(
            crate::wallet::spend::plan::PlanError::InsufficientFunds {
                available,
                required,
            },
        )))) => {
            assert_eq!(available, Zatoshis::from_u64(0).unwrap());
            assert_eq!(required, Zatoshis::from_u64(60_000).unwrap());
        }
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
    bump_and_check!(o: 0 i: 0 s: 0 t: 470_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 12 shield: 15_000 (1 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 0 i: 455_000 s: 0 t: 0);
    total_expected_fee += 15_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 13 orchard to own sapling: 20_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 10_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 425_000 s: 10_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 14 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 20_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 415_000 s: 10_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 15 orchard + sapling to own sapling: 20_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 405_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 0 s: 405_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 16 sapling self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 380_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 i: 0 s: 395_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);
}

/// Deterministic reproduction of issue #2450 (no live original: the
/// live shape is a load-dependent flake, three libtonode tests
/// failing whenever a slow validator response crosses the wallet's
/// send timeout). The first submission is accepted into the mock
/// mempool but its response is lost. The wallet's retry then receives
/// the validator's duplicate rejection, verbatim as zainod surfaces it
/// (zingolabs/zaino#1392). That rejection is proof of successful
/// transmission: the send must return Ok, the transaction must not be
/// marked Failed, and it must confirm with ordinary balance
/// arithmetic.
#[tokio::test]
async fn send_survives_lost_response_and_duplicate_rejection() {
    use crate::testutils::mock_indexer::LostSendDestination;
    use zingo_status::confirmation_status::ConfirmationStatus;

    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    recipient.set_transmit_retry_interval(std::time::Duration::ZERO);
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);

    net.chain.write().await.lose_next_send_response = Some(LostSendDestination::Mempool);

    let txids = from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .expect("a duplicate-in-mempool rejection proves transmission succeeded");

    // The wallet must not record the live transaction as Failed.
    {
        let wallet = recipient.wallet().read().await;
        for txid in txids.iter() {
            let status = wallet
                .wallet_transactions
                .get(txid)
                .expect("the transmitted transaction stays in the wallet")
                .status();
            assert!(
                !matches!(status, ConfirmationStatus::Failed(_)),
                "transaction {txid} marked Failed while live in the mempool"
            );
        }
    }

    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();
    // Identical arithmetic to `funded_send_confirms_on_the_mock_chain`:
    // the lost response and duplicate rejection must not perturb it.
    check_client_balances!(recipient, i: 70_000 o: 0 s: 0 t: 0);
}

/// Twin of [`send_survives_lost_response_and_duplicate_rejection`] for
/// the validator's earlier phase: the lost-response submission is
/// still in the download/verification queue when the retry arrives, so
/// the rejection reads "transaction dropped because it is already
/// queued for download" (zebra's pre-acceptance duplicate check),
/// observed live in the 2026-07-11 container runs, where verification
/// lagged the send by seconds under load. That rejection proves
/// delivery but not minability, so the wallet must hold success until
/// its probes see the storage-backed mempool rejection, and only then
/// return Ok, keeping send-Ok ⇒ minable-now. The mock answers two
/// queued rejections before promoting, so the probe loop is exercised
/// deterministically. Mining immediately after the send must therefore
/// confirm the transaction.
#[tokio::test]
async fn send_survives_lost_response_and_queued_duplicate_rejection() {
    use crate::testutils::mock_indexer::LostSendDestination;
    use zingo_status::confirmation_status::ConfirmationStatus;

    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    recipient.set_transmit_retry_interval(std::time::Duration::ZERO);
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);

    {
        let mut chain = net.chain.write().await;
        chain.lose_next_send_response = Some(LostSendDestination::DownloadQueue);
        chain.queued_rejections_before_promotion = 2;
    }

    let txids = from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .expect("probing resubmissions reach the storage-backed verdict");

    // The wallet must not record the delivered transaction as Failed.
    {
        let wallet = recipient.wallet().read().await;
        for txid in txids.iter() {
            let status = wallet
                .wallet_transactions
                .get(txid)
                .expect("the transmitted transaction stays in the wallet")
                .status();
            assert!(
                !matches!(status, ConfirmationStatus::Failed(_)),
                "transaction {txid} marked Failed while queued for download"
            );
        }
    }

    // send-Ok means minable NOW: mining immediately (the very race
    // that broke test_scanning_in_watch_only_mode live) must include
    // the transaction, with no verification-delay allowance.
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 70_000 o: 0 s: 0 t: 0);
}

/// A failed transmit inside a note-splitting round must not leave any
/// transaction stranded in `Calculated`. The immediate migration sibling
/// (`migrate_immediately`) fails every unsent transaction so the
/// notes it reserved become spendable again. The split round in
/// `migrate_to_ironwood` must enforce the same invariant, or the
/// transactions queued behind the failing one keep their notes marked
/// spent by transactions that never reached the network, and a replan
/// silently excludes that value until expiry self-heals it (~40 blocks
/// plus a sync).
///
/// Setup: 17 fabricated legacy-Orchard (V2) notes make the provisional
/// planner (16-action total budget) emit a first reduction round of several
/// merge transactions — more than one, so a failure of the first can strand
/// the rest. The mock indexer's lost-response fault plus an
/// effectively-infinite download-queue rejection budget makes the FIRST
/// submission fail deterministically after the wallet's probe budget. The
/// transactions behind it must not stay stranded.
#[tokio::test]
async fn failed_split_round_transmit_strands_calculated_transactions() {
    use zcash_primitives::transaction::TxId;
    use zip32::AccountId;

    use pepper_sync::wallet::{OrchardNote, OutputInterface as _};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::testutils::mock_indexer::LostSendDestination;
    use crate::testutils::synthetic_wallet::inject_confirmed_orchard_notes;

    const NOTES: u32 = 17;
    const NOTE_VALUE: u64 = 120_000;
    const TIP: u32 = 41;

    // A real mock-net client, synced over an empty chain so the wallet
    // carries genuine wallet blocks and scan state, then handed 34
    // spendable legacy-Orchard notes whose nullifiers are really derived,
    // so pepper-sync's spend detection marks them when the round spends
    // them.
    let mut net = MockNet::launch().await;
    net.chain.write().await.mine_empty_blocks(TIP);
    let mut client = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    client.set_transmit_retry_interval(std::time::Duration::ZERO);
    client
        .sync_and_await()
        .await
        .expect("initial sync succeeds");
    {
        let wallet_lock = client.wallet().clone();
        let mut wallet = wallet_lock.write().await;
        inject_confirmed_orchard_notes(&mut wallet, NOTES, NOTE_VALUE, TIP);
    }

    // Arm the deterministic transmit failure: the first send's response is
    // lost while the bytes sit in the validator's download queue, and the
    // queue never promotes, so every duplicate probe is rejected until the
    // wallet's probe budget is exhausted and it marks that transaction
    // Failed and errors out of transmit_transactions.
    {
        let mut chain = net.chain.write().await;
        chain.lose_next_send_response = Some(LostSendDestination::DownloadQueue);
        chain.queued_rejections_before_promotion = u8::MAX;
    }

    let err = client
        .migrate_to_ironwood(AccountId::ZERO)
        .await
        .expect_err("the first split transaction's transmit fails");
    eprintln!("migrate_to_ironwood returned: {err:?}");

    // Diagnostics and precondition: the round must have reached the
    // transmit stage (exactly one transaction Failed there).
    let wallet = client.wallet().read().await;
    let mut failed = Vec::new();
    let mut calculated = Vec::new();
    for tx in wallet.wallet_transactions.values() {
        match tx.status() {
            ConfirmationStatus::Calculated(_) => {
                let spent_inputs: Vec<(TxId, u64)> = wallet
                    .wallet_transactions
                    .values()
                    .flat_map(OrchardNote::transaction_outputs)
                    .filter(|note| note.spending_transaction() == Some(tx.txid()))
                    .map(|note| (note.output_id().txid(), note.value()))
                    .collect();
                eprintln!(
                    "stranded Calculated transaction {} spends {} notes still \
                     marked spent: {spent_inputs:?}",
                    tx.txid(),
                    spent_inputs.len(),
                );
                calculated.push(tx.txid());
            }
            ConfirmationStatus::Failed(_) => failed.push(tx.txid()),
            _ => (),
        }
    }
    assert!(
        !failed.is_empty(),
        "precondition: the transmit stage must have failed the first split \
         transaction (otherwise this test failed before transmit)"
    );

    // The invariant the immediate migration path enforces (fail_unsent_transactions) and
    // the split path must too: after a failed round, nothing may remain
    // Calculated. Its notes would stay spent by transactions that will
    // never transmit, and a replan silently excludes them.
    assert!(
        calculated.is_empty(),
        "a failed note-split round stranded {} transaction(s) in Calculated \
         with their input notes marked spent: {calculated:?}",
        calculated.len()
    );
}

/// The offline twins whose assertions read the editorial surface.
#[cfg(feature = "perspective")]
mod perspective {
    use crate::lightclient::LightClient;
    use crate::perspective::value_transfer::{
        SelfSendValueTransfer, SentValueTransfer, ValueTransfer, ValueTransferKind, ValueTransfers,
    };
    use crate::testutils::synthetic_wallet::inject_confirmed_orchard_notes;

    use super::*;

    const NOTE_VALUE: u64 = 1_000_000;
    const TIP: u32 = 41;

    /// A real mock-net client, synced over an empty chain, handed one
    /// spendable legacy-Orchard note whose nullifier is really derived, so
    /// pepper-sync's spend detection marks it when a migration spends it
    /// and the summary sees the transaction as Orchard-funded.
    async fn orchard_funded_client() -> (MockNet, LightClient) {
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
        (net, client)
    }

    /// The first value transfer classified as an Orchard→Ironwood migration.
    fn migration_transfer(value_transfers: &ValueTransfers) -> Option<&ValueTransfer> {
        value_transfers.iter().find(|vt| {
            vt.kind
                == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                    SelfSendValueTransfer::Migration,
                ))
        })
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

    /// A confirmed Orchard→Ironwood immediate migration transaction must surface in the
    /// history as a `migration` value transfer, not `memo-to-self` and not
    /// `basic`. Its self-received Ironwood output carries the canonical empty
    /// memo (`MemoBytes::empty()`), so this pins the self-send classification
    /// order in `value_transfers()`: the migration predicate must win over the
    /// received-memo check regardless of how that memo decodes.
    #[tokio::test]
    async fn immediate_migration_is_a_migration_value_transfer() {
        use zip32::AccountId;

        let (net, mut client) = orchard_funded_client().await;

        let summary = client
            .migrate_immediately(AccountId::ZERO)
            .await
            .expect("the immediate migration builds and transmits");
        assert_eq!(
            summary.txids.len(),
            1,
            "one note migrates in one transaction"
        );

        net.chain.write().await.mine_mempool();
        client.sync_and_await().await.unwrap();

        let value_transfers = client.value_transfers(false).await.unwrap();
        assert!(
            migration_transfer(&value_transfers).is_some(),
            "the immediate migration transaction must classify as a migration value transfer; got {:?}",
            value_transfers.iter().map(|vt| vt.kind).collect::<Vec<_>>(),
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
        const MEMO: &str = "moving my own funds";

        let (_net, mut client) = orchard_funded_client().await;

        // A send to the wallet's own orchard receiver lands in the Ironwood pool
        // post-NU6.3, funded from the legacy Orchard note: an Orchard→Ironwood
        // self-send carrying a real memo. Asserted on the pending (transmitted)
        // record, the state the history shows right after transmission, and the
        // same classification path as a confirmed transaction. (Mining it would
        // conflict the injected note's fabricated orchard tree leaf with the
        // send's real orchard commitments at the same positions.)
        let own_ua = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
        from_inputs::quick_send(&mut client, vec![(&own_ua, 50_000, Some(MEMO))])
            .await
            .unwrap();

        let value_transfers = client.value_transfers(false).await.unwrap();
        let migration = migration_transfer(&value_transfers).unwrap_or_else(|| {
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
}

/// A mock-chain send travels the mixnet route and says so: the receipt
/// names the Correspondent that accepted the transaction and the
/// session's SOCKS5 endpoint, never the sync indexer. Mock-net clients
/// run with Mixnet Mode switched on, so the Correspondent draw, the
/// escalation rounds, and the cap all run for real; only the bytes take
/// the mock indexer's channel instead of the tunnel.
#[cfg(feature = "nym")]
#[tokio::test]
async fn a_mock_chain_send_reports_the_mixnet_route() {
    use crate::lightclient::send::TransmitRoute;

    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);

    let reports = from_inputs::quick_send_reported(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .unwrap();

    for report in &reports {
        match &report.route {
            TransmitRoute::Mixnet {
                correspondent,
                via_socks5,
            } => {
                assert_eq!(
                    via_socks5,
                    &crate::mocks::transmission::MOCK_SOCKS5_ADDR.to_string()
                );
                assert!(
                    crate::correspondent::CORRESPONDENT_INDEXERS
                        .iter()
                        .any(|entry| entry.contains(correspondent.as_str())),
                    "the winning Correspondent {correspondent} is not drawn from the curated pool"
                );
            }
            TransmitRoute::Clearnet { indexer } => {
                panic!("a mixnet-on session leaked the transmission to clearnet at {indexer}")
            }
        }
    }

    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // 100_000 funding minus the 20_000 payment and its 10_000 one-orchard-
    // spend, two-logical-action ZIP-317 fee.
    check_client_balances!(recipient, i: 70_000 o: 0 s: 0 t: 0);
}

/// The falsifier for [`a_mock_chain_send_reports_the_mixnet_route`]: the
/// deliberate toggle-off is the one act that routes a transmission over
/// clearnet as informed consent, and its receipt names the sync indexer
/// rather than a Correspondent.
#[cfg(feature = "nym")]
#[tokio::test]
async fn switching_the_mixnet_off_reports_the_clearnet_route() {
    use crate::lightclient::send::TransmitRoute;

    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();

    recipient.disable_mixnet().await;

    let reports = from_inputs::quick_send_reported(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .unwrap();

    for report in &reports {
        assert!(
            matches!(report.route, TransmitRoute::Clearnet { .. }),
            "a switched-off session reported {:?} instead of clearnet",
            report.route
        );
    }
}
