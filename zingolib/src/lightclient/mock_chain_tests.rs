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

use pepper_sync::sync::SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;
use pepper_sync::wallet::IronwoodNote;
use shardtree::store::ShardStore;
use zcash_address::ZcashAddress;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;

use crate::check_client_balances;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::error::ProposeSendError;
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

/// Returns a TEX-encoded taddr from an external wallet.
fn external_tex_address() -> String {
    use pepper_sync::keys::decode_address;
    use zcash_client_backend::address::Address;
    use zcash_transparent::address::TransparentAddress;

    let external_wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
    let taddr = external_wallet
        .transparent_addresses()
        .values()
        .next()
        .unwrap()
        .clone();
    let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
        decode_address(&external_wallet.chain_type(), &taddr).unwrap()
    else {
        panic!("a wallet-generated first taddr is p2pkh")
    };
    crate::testutils::interpret_taddr_as_tex_addr(taddr_bytes, &external_wallet.chain_type())
}

/// Mines one empty block, one funding block, and `extra_blocks` into every
/// chain in `nets`.
async fn fund_mirrored(
    nets: &[&MockNet],
    receivers: Vec<(&str, u64, Option<&str>)>,
    extra_blocks: u32,
) {
    let funding = faucet_funding_transaction(receivers).await;
    for net in nets {
        let mut chain = net.chain.write().await;
        chain.mine_empty_blocks(1);
        chain.mine_block(vec![funding.clone()]);
        chain.mine_empty_blocks(extra_blocks);
    }
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

/// Tests that max_send_value() returns a non-zero value for a wallet
/// with funds.
#[tokio::test]
async fn max_send_value_to_tex_empties_the_wallet() {
    let funding = 100_000;
    let mut net = MockNet::launch().await;
    let mut sender = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&sender_ua, funding, None)], 1).await;
    sender.sync_and_await().await.unwrap();
    check_client_balances!(sender, i: funding o: 0 s: 0 t: 0);

    let tex_address = external_tex_address();
    let max_send_value = sender
        .max_send_value(
            ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            false,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();
    assert!(
        max_send_value > Zatoshis::ZERO,
        "a funded wallet can send to a TEX address"
    );

    from_inputs::quick_send(
        &mut sender,
        vec![(&tex_address, max_send_value.into_u64(), None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    sender.sync_and_await().await.unwrap();

    check_client_balances!(sender, i: 0 o: 0 s: 0 t: 0);
}

/// Tests that max_send_value() returns a non-zero value for a wallet and that it works with zennies.
#[tokio::test]
async fn max_send_value_to_tex_with_zennies_empties_the_wallet() {
    let funding = 2 * crate::ZENNIES_FOR_ZINGO_AMOUNT;
    let mut net = MockNet::launch().await;
    let mut sender = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&sender_ua, funding, None)], 1).await;
    sender.sync_and_await().await.unwrap();
    check_client_balances!(sender, i: funding o: 0 s: 0 t: 0);

    let tex_address = external_tex_address();
    let max_send_value = sender
        .max_send_value(
            ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            true,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();
    assert!(
        max_send_value > Zatoshis::ZERO,
        "a wallet funded past the zenny can send to a TEX address"
    );

    let zenny_address = crate::get_zennies_for_zingo_address(sender.chain_type());
    from_inputs::quick_send(
        &mut sender,
        vec![
            (&tex_address, max_send_value.into_u64(), None),
            (zenny_address, crate::ZENNIES_FOR_ZINGO_AMOUNT, None),
        ],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    sender.sync_and_await().await.unwrap();

    check_client_balances!(sender, i: 0 o: 0 s: 0 t: 0);
}

/// Tests that max_send_value() to a shielded address, without zennies,
/// spends the whole balance across the ironwood and sapling pools.
#[tokio::test]
async fn max_send_value_to_shielded_empties_the_wallet() {
    let ironwood_funding = 100_000;
    let sapling_funding = 50_000;
    let mut net = MockNet::launch().await;
    let mut sender = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let sender_sapling = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Sapling)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(
        &net,
        vec![
            (&sender_ua, ironwood_funding, None),
            (&sender_sapling, sapling_funding, None),
        ],
        1,
    )
    .await;
    sender.sync_and_await().await.unwrap();
    check_client_balances!(sender, i: ironwood_funding o: 0 s: sapling_funding t: 0);

    let recipient = external_address(PoolType::ORCHARD);
    let max_send_value = sender
        .max_send_value(
            ZcashAddress::try_from_encoded(&recipient).unwrap(),
            false,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();
    assert!(
        max_send_value > Zatoshis::ZERO,
        "a funded wallet can send to a shielded address"
    );

    let one_zat = Zatoshis::const_from_u64(1);
    let past_max = (max_send_value + one_zat).unwrap();
    assert!(matches!(
        from_inputs::propose(&mut sender, vec![(&recipient, past_max.into_u64(), None)]).await,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds { .. }
        ))
    ));

    from_inputs::quick_send(
        &mut sender,
        vec![(&recipient, max_send_value.into_u64(), None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    sender.sync_and_await().await.unwrap();

    check_client_balances!(sender, i: 0 o: 0 s: 0 t: 0);
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
        chain.enter_mempool(incoming);
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
        Err(LightClientError::SendError(SendError::ProposeSendError(
            ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available,
                    required,
                },
            ),
        ))) => {
            assert_eq!(available, Zatoshis::from_u64(0).unwrap());
            assert_eq!(required, Zatoshis::from_u64(20_000).unwrap());
        }
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
    bump_and_check!(o: 0 i: 0 s: 0 t: 470_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 11 transparent-to-sapling likewise refused.
    match from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 50_000, None)]).await {
        Err(LightClientError::SendError(SendError::ProposeSendError(
            ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available,
                    required,
                },
            ),
        ))) => {
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
    {
        let mut chain = net.chain.write().await;
        chain.rules.anchors = false;
        chain.mine_empty_blocks(TIP);
    }
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
        {
            let mut chain = net.chain.write().await;
            chain.rules.anchors = false;
            chain.mine_empty_blocks(TIP);
        }
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
/// names the Destination that accepted the transaction and the
/// session's SOCKS5 endpoint. Mock-net clients run with Mixnet Mode
/// switched on, so the Destination draw, the escalation rounds, and the
/// cap all run for real; only the bytes take the mock indexer's channel
/// instead of the tunnel.
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
                destination,
                via_socks5,
            } => {
                assert_eq!(
                    via_socks5,
                    &crate::mocks::transmission::MOCK_SOCKS5_ADDR.to_string()
                );
                assert_eq!(
                    destination,
                    net.indexer_uri()
                        .host()
                        .expect("the mock indexer has a host"),
                    "a regtest draw names the sync indexer alone"
                );
            }
            TransmitRoute::Clearnet { destination } => {
                panic!("a mixnet-on session leaked the transmission to clearnet at {destination}")
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
/// rather than a Destination.
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

#[tokio::test]
async fn shardtree_roundtrip_restores_retained_checkpoints() {
    fn checkpoint_exists<S>(store: &S, height: u32) -> bool
    where
        S: ShardStore<CheckpointId = BlockHeight>,
    {
        store
            .get_checkpoint(&BlockHeight::from_u32(height))
            .expect("infallible")
            .is_some()
    }

    fn all_checkpoints_stored<S>(store: &S, chain_height: u32) -> bool
    where
        S: ShardStore<CheckpointId = BlockHeight>,
    {
        for boundary in [144, 288] {
            if !checkpoint_exists(store, boundary) {
                return false;
            }
        }
        for rolling_window in
            (chain_height - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1)..=chain_height
        {
            if !checkpoint_exists(store, rolling_window) {
                return false;
            }
        }

        true
    }

    fn all_boundaries_retained<S>(store: &S, chain_height: u32) -> bool
    where
        S: ShardStore<CheckpointId = BlockHeight>,
    {
        let retained = store.retained_checkpoints().unwrap();
        let no_of_boundaries = chain_height / 144;
        for boundary_index in 0..no_of_boundaries {
            let boundary = (boundary_index + 1) * 144;
            if !retained.contains(&BlockHeight::from_u32(boundary)) {
                return false;
            }
        }

        true
    }

    let mut chain_height = 500;
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;

    net.chain.write().await.mine_empty_blocks(chain_height - 2);
    recipient.sync_and_await().await.unwrap();

    // create shielded note commitments to trigger checkpoint pruning
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    recipient.sync_and_await().await.unwrap();

    {
        let shard_trees = &recipient.wallet().read().await.shard_trees;
        assert!(all_checkpoints_stored(
            shard_trees.sapling.store(),
            chain_height
        ));
        assert!(all_checkpoints_stored(
            shard_trees.orchard.store(),
            chain_height
        ));
        assert!(all_checkpoints_stored(
            shard_trees.ironwood.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.sapling.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.orchard.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.ironwood.store(),
            chain_height
        ));
    }

    recipient.save_task().await;
    recipient.wait_for_save().await;
    recipient.shutdown_save_task().await.unwrap();
    drop(recipient);

    let mut reloaded_recipient = net.client_from_file(0).await;
    // create shielded note commitments to trigger checkpoint pruning on first sync of reloaded client
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;
    chain_height += 2;
    reloaded_recipient.sync_and_await().await.unwrap();
    {
        let shard_trees = &reloaded_recipient.wallet().read().await.shard_trees;
        assert!(all_checkpoints_stored(
            shard_trees.sapling.store(),
            chain_height
        ));
        assert!(all_checkpoints_stored(
            shard_trees.orchard.store(),
            chain_height
        ));
        assert!(all_checkpoints_stored(
            shard_trees.ironwood.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.sapling.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.orchard.store(),
            chain_height
        ));
        assert!(all_boundaries_retained(
            shard_trees.ironwood.store(),
            chain_height
        ));
    }
}

mod strict_chain {
    use std::time::Duration;

    use zaino_proto::tonic::Code;
    use zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS;
    use zcash_protocol::value::Zatoshis;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use pepper_sync::wallet::KeyIdInterface;
    use pepper_sync::wallet::traits::SyncWallet;

    use crate::lightclient::LightClient;
    use crate::testutils::chain_generics::fixtures;
    use crate::testutils::mock_indexer::{Fault, LostSendDestination, Rpc};

    use super::*;

    const FUNDING: u64 = 100_000;
    const PAYMENT: u64 = 10_000;
    const REWARD_ZATS: u64 = 1_000_000;
    const REWARD: Zatoshis = Zatoshis::const_from_u64(REWARD_ZATS);

    async fn funded_sender(net: &mut MockNet) -> LightClient {
        let mut sender = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        sender.set_transmit_retry_interval(Duration::ZERO);
        let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
        fund(net, vec![(&sender_ua, FUNDING, None)], 1).await;
        sender.sync_and_await().await.unwrap();
        check_client_balances!(sender, i: FUNDING o: 0 s: 0 t: 0);
        sender
    }

    #[tokio::test]
    async fn rejected_transaction_gets_failed_status() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        from_inputs::propose(
            &mut sender,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        let calculated = sender.calculate_stored_proposal().await.unwrap();
        let expiry = {
            let wallet = sender.wallet().read().await;
            wallet
                .wallet_transactions
                .get(&calculated[0])
                .unwrap()
                .transaction()
                .expiry_height()
        };
        {
            let mut chain = net.chain.write().await;
            let tip = chain.tip();
            chain.mine_empty_blocks(u32::from(expiry) - tip);
        }

        let err = sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("expired"), "{err:?}");
        let wallet = sender.wallet().read().await;
        assert!(matches!(
            wallet
                .wallet_transactions
                .get(&calculated[0])
                .unwrap()
                .status(),
            ConfirmationStatus::Failed(_)
        ));
        assert_eq!(net.chain.read().await.mempool_len(), 0);
    }

    #[tokio::test]
    async fn transmitted_transaction_has_mempool_status_before_mining() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let txids = from_inputs::quick_send(
            &mut sender,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        assert_eq!(net.chain.read().await.mempool_len(), 1);

        sender.sync_and_await().await.unwrap();
        let summaries = sender.transaction_summaries(false).await.unwrap();
        let sent = summaries
            .iter()
            .find(|summary| summary.txid == txids[0])
            .unwrap();
        assert!(
            matches!(sent.status, ConfirmationStatus::Mempool(_)),
            "{:?}",
            sent.status
        );

        net.chain.write().await.mine_mempool();
        sender.sync_and_await().await.unwrap();
        let summaries = sender.transaction_summaries(false).await.unwrap();
        let sent = summaries
            .iter()
            .find(|summary| summary.txid == txids[0])
            .unwrap();
        assert!(matches!(sent.status, ConfirmationStatus::Confirmed(_)));
    }

    #[tokio::test]
    async fn sync_recovers_from_a_truncated_block_stream() {
        let mut net = MockNet::launch().await;
        let mut recipient = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        fund(&net, vec![(&recipient_ua, FUNDING, None)], 3).await;
        net.chain
            .write()
            .await
            .faults
            .inject(Rpc::BlockRange, Fault::TruncateStream { after: 1 });

        recipient.sync_and_await().await.unwrap();
        assert_eq!(net.chain.read().await.faults.pending(Rpc::BlockRange), 0);
        check_client_balances!(recipient, i: FUNDING o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn sync_fails_while_the_indexer_is_unavailable() {
        let mut net = MockNet::launch().await;
        let mut client = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        net.chain.write().await.mine_empty_blocks(2);
        {
            let mut chain = net.chain.write().await;
            for _ in 0..5 {
                chain.faults.inject(
                    Rpc::LatestBlock,
                    Fault::Fail(Code::Unavailable, "mock outage".to_string()),
                );
            }
        }
        assert!(client.sync_and_await().await.is_err());
    }

    #[tokio::test]
    async fn coinbase_reward_becomes_spendable_after_maturity() {
        let mut net = MockNet::launch().await;
        let mut miner = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        miner.set_transmit_retry_interval(Duration::ZERO);
        let miner_taddr = get_base_address(&miner, PoolType::Transparent).await;
        let coinbase = {
            let mut chain = net.chain.write().await;
            let coinbase = chain.mine_block_rewarding(&miner_taddr, REWARD, vec![]);
            chain.mine_empty_blocks(1);
            coinbase
        };
        miner.sync_and_await().await.unwrap();
        check_client_balances!(miner, i: 0 o: 0 s: 0 t: 0);
        let summaries = miner.transaction_summaries(false).await.unwrap();
        assert!(summaries.iter().any(|summary| summary.value == REWARD_ZATS
            && summary.status == ConfirmationStatus::Confirmed(BlockHeight::from_u32(1))));
        assert!(!coinbase.is_empty());

        net.chain
            .write()
            .await
            .mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
        miner.sync_and_await().await.unwrap();
        check_client_balances!(miner, i: 0 o: 0 s: 0 t: REWARD_ZATS);

        miner.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        assert_eq!(net.chain.read().await.mempool_len(), 1);
        net.chain.write().await.mine_mempool();
        miner.sync_and_await().await.unwrap();
        let balance = miner.account_balance(zip32::AccountId::ZERO).await.unwrap();
        assert_eq!(balance.confirmed_transparent_balance.unwrap().into_u64(), 0);
        assert!(balance.total_ironwood_balance.unwrap().into_u64() > 0);
    }

    #[tokio::test]
    async fn generate_a_range_of_value_transfers_on_the_mock_chain() {
        fixtures::create_various_value_transfers::<MockNet>().await;
    }

    #[tokio::test]
    async fn send_shield_cycle_on_the_mock_chain() {
        fixtures::send_shield_cycle::<MockNet>(1).await;
    }

    use nonempty::NonEmpty;
    use pepper_sync::keys::transparent::TransparentScope;
    use pepper_sync::wallet::OutputInterface;
    use zcash_client_backend::data_api::OutputLockStore;
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_primitives::transaction::TxId;

    use crate::wallet::LightWallet;

    const RETRIES_BEFORE_DELIVERY_CHECK: usize = 4;
    const ONE_INPUT_SEND_FEE: u64 = 10_000;
    const QUEUED_REJECTIONS: usize = 40;
    const ZAINO_EXPIRED_MESSAGE: &str = "unhandled rpc-specific zaino_fetch::jsonrpsee::response::SendTransactionError error: RPC Error (code: -26): transaction has expired";

    fn expected_after_one_send() -> u64 {
        FUNDING - PAYMENT - ONE_INPUT_SEND_FEE
    }

    fn ironwood_notes_unspent(wallet: &LightWallet) -> bool {
        wallet
            .wallet_transactions
            .values()
            .flat_map(IronwoodNote::transaction_outputs)
            .all(|note| note.spending_transaction().is_none())
    }

    fn refund_address_count(wallet: &LightWallet) -> usize {
        wallet
            .transparent_addresses()
            .keys()
            .filter(|id| id.scope() == TransparentScope::Refund)
            .count()
    }

    async fn status_of(client: &LightClient, txid: &TxId) -> ConfirmationStatus {
        client
            .wallet()
            .read()
            .await
            .wallet_transactions
            .get(txid)
            .unwrap()
            .status()
    }

    async fn calculated_send(sender: &mut LightClient) -> NonEmpty<TxId> {
        from_inputs::propose(
            sender,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        sender.calculate_stored_proposal().await.unwrap()
    }

    async fn expiry_of(sender: &LightClient, txid: &TxId) -> BlockHeight {
        sender
            .wallet()
            .read()
            .await
            .wallet_transactions
            .get(txid)
            .unwrap()
            .transaction()
            .expiry_height()
    }

    async fn transmit_after_expiry(net: &MockNet, sender: &mut LightClient) -> NonEmpty<TxId> {
        let calculated = calculated_send(sender).await;
        let expiry = expiry_of(sender, &calculated[0]).await;
        {
            let mut chain = net.chain.write().await;
            let tip = chain.tip();
            chain.mine_empty_blocks(u32::from(expiry) - tip);
        }
        sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            status_of(sender, &calculated[0]).await,
            ConfirmationStatus::Failed(_)
        ));
        calculated
    }

    fn tex_request() -> TransactionRequest {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_transparent::address::TransparentAddress;

        let external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let taddr = external_wallet
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone();
        let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
            decode_address(&external_wallet.chain_type(), &taddr).unwrap()
        else {
            panic!("a wallet-generated first taddr is p2pkh")
        };
        let tex_address = crate::testutils::interpret_taddr_as_tex_addr(
            taddr_bytes,
            &external_wallet.chain_type(),
        );
        TransactionRequest::new(vec![Payment::without_memo(
            zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            Zatoshis::const_from_u64(PAYMENT),
        )])
        .unwrap()
    }

    async fn calculated_tex_send(sender: &mut LightClient) -> NonEmpty<TxId> {
        sender
            .propose_send(tex_request(), zip32::AccountId::ZERO)
            .await
            .unwrap();
        let calculated = sender.calculate_stored_proposal().await.unwrap();
        assert_eq!(calculated.len(), 2);
        calculated
    }

    fn inject_send_failures(chain: &mut crate::testutils::mock_indexer::MockChain, count: usize) {
        for _ in 0..count {
            chain.faults.inject(
                Rpc::SendTransaction,
                Fault::Fail(Code::Unavailable, "mock outage".to_string()),
            );
        }
    }

    #[tokio::test]
    async fn accepted_send_marked_failed_is_confirmed_after_mining() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let calculated = calculated_send(&mut sender).await;
        {
            let mut chain = net.chain.write().await;
            chain.lose_next_send_response = Some(LostSendDestination::Mempool);
            chain
                .faults
                .inject(Rpc::SendTransaction, Fault::Delay(Duration::ZERO));
            inject_send_failures(&mut chain, QUEUED_REJECTIONS);
        }

        sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Failed(_)
        ));
        assert_eq!(net.chain.read().await.mempool_len(), 1);
        assert!(ironwood_notes_unspent(&*sender.wallet().read().await));

        net.chain.write().await.mine_mempool();
        sender.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Confirmed(_)
        ));
        let expected = expected_after_one_send();
        check_client_balances!(sender, i: expected o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn released_inputs_are_reselected_after_a_rejection() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        transmit_after_expiry(&net, &mut sender).await;
        assert!(ironwood_notes_unspent(&*sender.wallet().read().await));

        sender.sync_and_await().await.unwrap();
        let resent = from_inputs::quick_send(
            &mut sender,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        assert_eq!(net.chain.read().await.mempool_len(), 1);
        net.chain.write().await.mine_mempool();
        sender.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&sender, &resent[0]).await,
            ConfirmationStatus::Confirmed(_)
        ));
        let expected = expected_after_one_send();
        check_client_balances!(sender, i: expected o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn later_step_rejected_leaves_earlier_step_transmitted() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let calculated = calculated_tex_send(&mut sender).await;
        {
            let mut chain = net.chain.write().await;
            chain
                .faults
                .inject(Rpc::SendTransaction, Fault::Delay(Duration::ZERO));
            inject_send_failures(&mut chain, QUEUED_REJECTIONS);
        }

        sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Transmitted(_)
        ));
        assert!(matches!(
            status_of(&sender, &calculated[1]).await,
            ConfirmationStatus::Failed(_)
        ));
        assert_eq!(net.chain.read().await.mempool_len(), 1);

        net.chain.write().await.mine_mempool();
        sender.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Confirmed(_)
        ));
        assert!(matches!(
            status_of(&sender, &calculated[1]).await,
            ConfirmationStatus::Failed(_)
        ));
        let balance = sender
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert!(balance.confirmed_transparent_balance.unwrap().into_u64() > 0);
    }

    async fn failed_first_step_via_quick_send(net: &MockNet, sender: &mut LightClient) {
        inject_send_failures(&mut *net.chain.write().await, QUEUED_REJECTIONS);
        sender
            .quick_send(tex_request(), zip32::AccountId::ZERO, true)
            .await
            .unwrap_err();
    }

    fn steps_by_status(wallet: &LightWallet) -> (Vec<TxId>, Vec<TxId>) {
        let mut failed = Vec::new();
        let mut calculated = Vec::new();
        for transaction in wallet.wallet_transactions.values() {
            match transaction.status() {
                ConfirmationStatus::Failed(_) => failed.push(transaction.txid()),
                ConfirmationStatus::Calculated(_) => calculated.push(transaction.txid()),
                _ => (),
            }
        }
        (failed, calculated)
    }

    fn refund_address_ids(
        wallet: &LightWallet,
        account_id: zip32::AccountId,
    ) -> Vec<pepper_sync::keys::transparent::TransparentAddressId> {
        wallet
            .transparent_addresses()
            .keys()
            .filter(|id| id.scope() == TransparentScope::Refund && id.account_id() == account_id)
            .copied()
            .collect()
    }

    async fn confirmed_tex_send(net: &MockNet, sender: &mut LightClient) -> NonEmpty<TxId> {
        let steps = sender
            .quick_send(tex_request(), zip32::AccountId::ZERO, true)
            .await
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(net.chain.read().await.mempool_len(), 2);
        net.chain.write().await.mine_mempool();
        sender.sync_and_await().await.unwrap();
        steps
    }

    #[tokio::test]
    async fn tex_send_confirms_on_the_strict_mock_chain() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let steps = confirmed_tex_send(&net, &mut sender).await;
        for txid in &steps {
            assert!(matches!(
                status_of(&sender, txid).await,
                ConfirmationStatus::Confirmed(_)
            ));
        }
        let wallet = sender.wallet().read().await;
        assert_eq!(refund_address_ids(&wallet, zip32::AccountId::ZERO).len(), 1);
        assert_eq!(net.chain.read().await.mempool_len(), 0);
    }

    #[tokio::test]
    async fn failed_send_in_one_account_keeps_refund_addresses_of_other_accounts() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        confirmed_tex_send(&net, &mut sender).await;
        let other_account = zip32::AccountId::try_from(1).unwrap();
        let other_ua = {
            let mut wallet = sender.wallet().write().await;
            wallet.create_new_account().unwrap();
            let (_, address) = wallet
                .generate_unified_address(ReceiverSelection::all_shielded(), other_account)
                .unwrap();
            address.encode(&wallet.chain_type())
        };
        fund(&net, vec![(&other_ua, FUNDING, None)], 1).await;
        sender.sync_and_await().await.unwrap();
        assert_eq!(
            sender
                .account_balance(other_account)
                .await
                .unwrap()
                .confirmed_ironwood_balance
                .unwrap()
                .into_u64(),
            FUNDING
        );
        let first_account_refunds_before =
            refund_address_ids(&*sender.wallet().read().await, zip32::AccountId::ZERO);
        assert_eq!(first_account_refunds_before.len(), 1);

        inject_send_failures(&mut *net.chain.write().await, QUEUED_REJECTIONS);
        sender
            .quick_send(tex_request(), other_account, true)
            .await
            .unwrap_err();

        let wallet = sender.wallet().read().await;
        assert_eq!(
            refund_address_ids(&wallet, zip32::AccountId::ZERO),
            first_account_refunds_before
        );
        assert!(refund_address_ids(&wallet, other_account).is_empty());
    }

    #[tokio::test]
    async fn failed_first_step_via_quick_send_truncates_refund_addresses() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let refund_addresses_before = refund_address_count(&*sender.wallet().read().await);
        failed_first_step_via_quick_send(&net, &mut sender).await;
        let wallet = sender.wallet().read().await;
        assert!(ironwood_notes_unspent(&wallet));
        assert_eq!(refund_address_count(&wallet), refund_addresses_before);
        assert_eq!(net.chain.read().await.mempool_len(), 0);
    }

    #[tokio::test]
    async fn failed_first_step_via_quick_send_fails_the_second_step() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        failed_first_step_via_quick_send(&net, &mut sender).await;
        let (failed, calculated) = steps_by_status(&*sender.wallet().read().await);
        assert_eq!(failed.len(), 2);
        assert!(
            calculated.is_empty(),
            "the second step must not stay Calculated after the first step failed: {calculated:?}"
        );
    }

    #[tokio::test]
    async fn failed_first_step_via_transmit_calculated_truncates_refund_addresses() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let refund_addresses_before = refund_address_count(&*sender.wallet().read().await);
        let calculated = calculated_tex_send(&mut sender).await;
        inject_send_failures(&mut *net.chain.write().await, QUEUED_REJECTIONS);
        sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Failed(_)
        ));
        let wallet = sender.wallet().read().await;
        assert!(ironwood_notes_unspent(&wallet));
        assert_eq!(refund_address_count(&wallet), refund_addresses_before);
    }

    #[tokio::test]
    async fn output_locks_are_released_after_a_rejection() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        transmit_after_expiry(&net, &mut sender).await;
        let wallet = sender.wallet().read().await;
        assert!(
            wallet
                .get_locked_outputs(zip32::AccountId::ZERO)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_send_survives_a_wallet_file_round_trip() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let calculated = transmit_after_expiry(&net, &mut sender).await;
        sender.save_task().await;
        sender.wait_for_save().await;
        sender.shutdown_save_task().await.unwrap();
        drop(sender);

        let mut reloaded = net.client_from_file(0).await;
        reloaded.set_transmit_retry_interval(Duration::ZERO);
        assert!(matches!(
            status_of(&reloaded, &calculated[0]).await,
            ConfirmationStatus::Failed(_)
        ));
        assert!(ironwood_notes_unspent(&*reloaded.wallet().read().await));

        reloaded.sync_and_await().await.unwrap();
        from_inputs::quick_send(
            &mut reloaded,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        net.chain.write().await.mine_mempool();
        reloaded.sync_and_await().await.unwrap();
        let expected = expected_after_one_send();
        check_client_balances!(reloaded, i: expected o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn a_rejection_is_retried_three_times_before_failing() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let calculated = calculated_send(&mut sender).await;
        {
            let mut chain = net.chain.write().await;
            for _ in 0..QUEUED_REJECTIONS {
                chain.faults.inject(
                    Rpc::SendTransaction,
                    Fault::Fail(Code::Internal, ZAINO_EXPIRED_MESSAGE.to_string()),
                );
            }
        }

        sender
            .transmit_calculated(calculated.clone())
            .await
            .unwrap_err();
        let attempts =
            QUEUED_REJECTIONS - net.chain.read().await.faults.pending(Rpc::SendTransaction);
        assert!(attempts > 0);
        assert_eq!(attempts % RETRIES_BEFORE_DELIVERY_CHECK, 0);
        assert!(matches!(
            status_of(&sender, &calculated[0]).await,
            ConfirmationStatus::Failed(_)
        ));
    }

    use zcash_primitives::transaction::Transaction;
    use zcash_protocol::consensus::BranchId;

    const BLOCK_FETCH_FAILURES: usize = 8;

    fn expiry_of_bytes(net: &MockNet, bytes: &[u8]) -> BlockHeight {
        Transaction::read(
            bytes,
            BranchId::for_height(&net.chain_type(), BlockHeight::from_u32(1)),
        )
        .unwrap()
        .expiry_height()
    }

    async fn pending_txid(client: &LightClient) -> TxId {
        client
            .wallet()
            .read()
            .await
            .wallet_transactions
            .values()
            .find(|transaction| transaction.status().is_pending())
            .unwrap()
            .txid()
    }

    async fn mine_past_expiry(net: &MockNet, expiry: BlockHeight) {
        let mut chain = net.chain.write().await;
        chain.mine_mempool();
        let tip = chain.tip();
        chain.mine_empty_blocks(u32::from(expiry) - tip);
    }

    async fn fail_next_block_fetches(net: &MockNet) {
        let mut chain = net.chain.write().await;
        for _ in 0..BLOCK_FETCH_FAILURES {
            chain.faults.inject(
                Rpc::BlockRange,
                Fault::Fail(Code::Unavailable, "mock outage".to_string()),
            );
        }
    }

    #[tokio::test]
    async fn received_transaction_is_not_failed_by_a_session_that_never_scans_its_block() {
        let mut net = MockNet::launch().await;
        let mut recipient = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        net.chain.write().await.mine_empty_blocks(1);
        recipient.sync_and_await().await.unwrap();

        let funding = faucet_funding_transaction(vec![(&recipient_ua, FUNDING, None)]).await;
        let expiry = expiry_of_bytes(&net, &funding);
        net.chain.write().await.enter_mempool(funding);
        recipient.sync_and_await().await.unwrap();
        let txid = pending_txid(&recipient).await;
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Mempool(_)
        ));

        mine_past_expiry(&net, expiry).await;
        fail_next_block_fetches(&net).await;
        assert!(recipient.sync_and_await().await.is_err());
        assert!(
            !matches!(
                status_of(&recipient, &txid).await,
                ConfirmationStatus::Failed(_)
            ),
            "a received transaction mined in an unscanned block was marked Failed"
        );

        net.chain.write().await.faults.clear(Rpc::BlockRange);
        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Confirmed(_)
        ));
        check_client_balances!(recipient, i: FUNDING o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn received_transaction_that_expires_unmined_is_failed() {
        let mut net = MockNet::launch().await;
        let mut recipient = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        net.chain.write().await.mine_empty_blocks(1);
        recipient.sync_and_await().await.unwrap();

        let funding = faucet_funding_transaction(vec![(&recipient_ua, FUNDING, None)]).await;
        let expiry = expiry_of_bytes(&net, &funding);
        net.chain.write().await.enter_mempool(funding);
        recipient.sync_and_await().await.unwrap();
        let txid = pending_txid(&recipient).await;
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Mempool(_)
        ));

        {
            let mut chain = net.chain.write().await;
            let tip = chain.tip();
            chain.mine_empty_blocks(u32::from(expiry) - tip);
            assert_eq!(chain.mempool_len(), 0);
        }
        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Failed(_)
        ));
        check_client_balances!(recipient, i: 0 o: 0 s: 0 t: 0);

        net.chain.write().await.mine_empty_blocks(1);
        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Failed(_)
        ));
    }

    #[tokio::test]
    async fn mined_send_inputs_stay_spent_through_a_session_that_never_scans_its_block() {
        let mut net = MockNet::launch().await;
        let mut sender = funded_sender(&mut net).await;
        let sent = from_inputs::quick_send(
            &mut sender,
            vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
        )
        .await
        .unwrap();
        let expiry = expiry_of(&sender, &sent[0]).await;

        mine_past_expiry(&net, expiry).await;
        fail_next_block_fetches(&net).await;
        assert!(sender.sync_and_await().await.is_err());
        assert!(
            !matches!(
                status_of(&sender, &sent[0]).await,
                ConfirmationStatus::Failed(_)
            ),
            "a mined send in an unscanned block was marked Failed"
        );
        assert!(!ironwood_notes_unspent(&*sender.wallet().read().await));
        let expected = expected_after_one_send();
        check_client_balances!(sender, i: expected o: 0 s: 0 t: 0);

        net.chain.write().await.faults.clear(Rpc::BlockRange);
        sender.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&sender, &sent[0]).await,
            ConfirmationStatus::Confirmed(_)
        ));
        check_client_balances!(sender, i: expected o: 0 s: 0 t: 0);
    }

    const HELD_FETCH: Duration = Duration::from_secs(8);
    const TRANSACTION_FETCH_FAILURES: usize = 8;
    const HOLD_POLL: Duration = Duration::from_millis(10);
    const HOLD_WAIT_LIMIT: Duration = Duration::from_secs(4);
    const BLOCKS_BELOW_EXPIRY_AT_LAST_SYNC: u32 = 5;
    const REORG_VERIFY_BLOCKS: u32 = 10;

    async fn fully_scanned_height(client: &LightClient) -> BlockHeight {
        client
            .wallet()
            .read()
            .await
            .get_sync_state()
            .unwrap()
            .fully_scanned_height()
            .unwrap()
    }

    async fn wait_until_scanned_through(client: &LightClient, height: BlockHeight) {
        let started = tokio::time::Instant::now();
        while fully_scanned_height(client).await < height {
            assert!(
                started.elapsed() < HOLD_WAIT_LIMIT,
                "the sync engine did not scan through {height} in time"
            );
            tokio::time::sleep(HOLD_POLL).await;
        }
    }

    async fn wait_until_fetch_is_held(net: &MockNet) {
        let started = tokio::time::Instant::now();
        while net.chain.read().await.faults.pending(Rpc::Transaction) > 0 {
            assert!(
                started.elapsed() < HOLD_WAIT_LIMIT,
                "the sync engine did not request the transaction in time"
            );
            tokio::time::sleep(HOLD_POLL).await;
        }
    }

    /// Syncs a recipient whose last known chain height is a few blocks below
    /// the expiry of a funding transaction that is still in the mempool.
    async fn mempool_transaction_near_expiry(
        net: &mut MockNet,
    ) -> (LightClient, String, TxId, BlockHeight) {
        let mut recipient = net
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        net.chain.write().await.mine_empty_blocks(1);
        recipient.sync_and_await().await.unwrap();

        let funding = faucet_funding_transaction(vec![(&recipient_ua, FUNDING, None)]).await;
        let expiry = expiry_of_bytes(net, &funding);
        {
            let mut chain = net.chain.write().await;
            chain.enter_mempool(funding);
            let last_sync_height = u32::from(expiry) - BLOCKS_BELOW_EXPIRY_AT_LAST_SYNC;
            let tip = chain.tip();
            assert!(last_sync_height > tip);
            chain.mine_empty_blocks(last_sync_height - tip);
        }
        recipient.sync_and_await().await.unwrap();
        let txid = pending_txid(&recipient).await;
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Mempool(_)
        ));
        (recipient, recipient_ua, txid, expiry)
    }

    /// Mines past `expiry`, then funds `recipient_ua` in a block above the
    /// reorg verification range so that block is scanned by a separate task
    /// after the expiry height is already scanned.
    async fn mine_past_expiry_and_fund_above_verify_range(
        net: &MockNet,
        recipient_ua: &str,
        expiry: BlockHeight,
    ) {
        {
            let mut chain = net.chain.write().await;
            let verify_end =
                u32::from(expiry) - BLOCKS_BELOW_EXPIRY_AT_LAST_SYNC + REORG_VERIFY_BLOCKS;
            let tip = chain.tip();
            chain.mine_empty_blocks(verify_end - tip);
            assert_eq!(chain.mempool_len(), 0);
        }
        fund(net, vec![(recipient_ua, FUNDING, None)], 0).await;
    }

    #[tokio::test]
    async fn stopped_session_that_scanned_past_expiry_fails_the_transaction() {
        let mut net = MockNet::launch().await;
        let (mut recipient, recipient_ua, txid, expiry) =
            mempool_transaction_near_expiry(&mut net).await;
        mine_past_expiry_and_fund_above_verify_range(&net, &recipient_ua, expiry).await;
        net.chain
            .write()
            .await
            .faults
            .inject(Rpc::Transaction, Fault::Delay(HELD_FETCH));

        recipient.sync().await.unwrap();
        wait_until_scanned_through(&recipient, expiry).await;
        recipient.stop_sync().unwrap();
        recipient.await_sync().await.unwrap();
        assert!(fully_scanned_height(&recipient).await >= expiry);
        assert!(
            matches!(
                status_of(&recipient, &txid).await,
                ConfirmationStatus::Failed(_)
            ),
            "a stopped session scanned past the expiry height without failing the transaction"
        );

        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Failed(_)
        ));
        check_client_balances!(recipient, i: FUNDING o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn failed_session_that_scanned_past_expiry_fails_the_transaction() {
        let mut net = MockNet::launch().await;
        let (mut recipient, recipient_ua, txid, expiry) =
            mempool_transaction_near_expiry(&mut net).await;
        mine_past_expiry_and_fund_above_verify_range(&net, &recipient_ua, expiry).await;
        {
            let mut chain = net.chain.write().await;
            for _ in 0..TRANSACTION_FETCH_FAILURES {
                chain.faults.inject(
                    Rpc::Transaction,
                    Fault::Fail(Code::Unavailable, "mock outage".to_string()),
                );
            }
        }

        assert!(recipient.sync_and_await().await.is_err());
        assert!(fully_scanned_height(&recipient).await >= expiry);
        assert!(
            matches!(
                status_of(&recipient, &txid).await,
                ConfirmationStatus::Failed(_)
            ),
            "a failed session scanned past the expiry height without failing the transaction"
        );

        net.chain.write().await.faults.clear(Rpc::Transaction);
        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Failed(_)
        ));
        check_client_balances!(recipient, i: FUNDING o: 0 s: 0 t: 0);
    }

    #[tokio::test]
    async fn stopped_session_does_not_fail_a_transaction_mined_in_an_unscanned_block() {
        let mut net = MockNet::launch().await;
        let (mut recipient, _, txid, expiry) = mempool_transaction_near_expiry(&mut net).await;
        {
            let mut chain = net.chain.write().await;
            let tip = chain.tip();
            chain.mine_empty_blocks(u32::from(expiry) - 1 - tip);
            chain.mine_mempool();
            assert_eq!(chain.tip(), u32::from(expiry));
        }
        net.chain
            .write()
            .await
            .faults
            .inject(Rpc::Transaction, Fault::Delay(HELD_FETCH));

        recipient.sync().await.unwrap();
        wait_until_fetch_is_held(&net).await;
        recipient.stop_sync().unwrap();
        recipient.await_sync().await.unwrap();
        assert!(fully_scanned_height(&recipient).await < expiry);
        assert!(
            !matches!(
                status_of(&recipient, &txid).await,
                ConfirmationStatus::Failed(_)
            ),
            "a transaction mined in an unscanned block was marked Failed by a stopped session"
        );

        recipient.sync_and_await().await.unwrap();
        assert!(matches!(
            status_of(&recipient, &txid).await,
            ConfirmationStatus::Confirmed(_)
        ));
        check_client_balances!(recipient, i: FUNDING o: 0 s: 0 t: 0);
    }
}

/// The mainnet broadcast rule, end to end over mock indexers.
mod mainnet_broadcast_offline {
    use super::*;
    use crate::destination::servers::{DestinationServerSet, IndexerConfig, Location, Role, Trust};
    use crate::lightclient::LightClient;
    use crate::lightclient::error::LightClientError;
    use crate::lightclient::send::{TransmitReport, TransmitRoute};
    use nonempty::NonEmpty;

    const SUPPRESSOR: &str = "suppressor.example";
    const ACCEPTOR: &str = "acceptor.example";

    struct Stage {
        sync: MockNet,
        suppressing: MockNet,
        accepting: MockNet,
    }

    impl Stage {
        fn set_over(&self, entries: &[(&MockNet, &str)]) -> DestinationServerSet {
            DestinationServerSet::registry_for_tests(
                Trust::Untrusted,
                entries
                    .iter()
                    .map(|(net, operator)| (net.indexer_uri(), *operator)),
            )
            .with_indexer(IndexerConfig::new(self.sync.indexer_uri()).location(Location::Remote))
        }

        async fn received(&self) -> (usize, usize, u32) {
            (
                self.sync.chain.read().await.mempool_len(),
                self.accepting.chain.read().await.mempool_len(),
                self.suppressing.chain.read().await.rejected_sends,
            )
        }
    }

    async fn stage() -> (Stage, LightClient) {
        let mut sync = MockNet::launch().await;
        let suppressing = MockNet::launch().await;
        suppressing.chain.write().await.reject_all_sends = true;
        let accepting = MockNet::launch().await;

        let mut recipient = sync
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        fund_mirrored(
            &[&sync, &suppressing, &accepting],
            vec![(&recipient_ua, 100_000, None)],
            1,
        )
        .await;
        recipient.sync_and_await().await.unwrap();
        recipient.set_transmit_retry_interval(std::time::Duration::from_millis(10));
        (
            Stage {
                sync,
                suppressing,
                accepting,
            },
            recipient,
        )
    }

    async fn send(
        recipient: &mut LightClient,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        from_inputs::quick_send_reported(
            recipient,
            vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
        )
        .await
    }

    #[tokio::test]
    async fn a_clearnet_send_goes_to_the_untrusted_sync_indexer_alone() {
        let (stage, mut recipient) = stage().await;
        #[cfg(feature = "nym")]
        recipient.disable_mixnet().await;
        recipient.set_destination_servers_for_tests(stage.set_over(&[
            (&stage.suppressing, SUPPRESSOR),
            (&stage.accepting, ACCEPTOR),
        ]));

        let reports = send(&mut recipient).await.unwrap();

        assert!(
            reports
                .iter()
                .all(|report| matches!(report.route, TransmitRoute::Clearnet { .. }))
        );
        assert_eq!(stage.received().await, (reports.len(), 0, 0));
    }

    #[tokio::test]
    async fn a_trusted_broadcast_indexer_receives_the_send_alone() {
        let (stage, mut recipient) = stage().await;
        #[cfg(feature = "nym")]
        recipient.disable_mixnet().await;
        recipient.set_destination_servers_for_tests(
            stage
                .set_over(&[(&stage.suppressing, SUPPRESSOR)])
                .with_indexer(
                    IndexerConfig::new(stage.accepting.indexer_uri())
                        .role(Role::Broadcast)
                        .trust(Trust::Trusted)
                        .location(Location::Remote),
                ),
        );

        let reports = send(&mut recipient).await.unwrap();

        assert_eq!(stage.received().await, (0, reports.len(), 0));
    }

    #[cfg(feature = "nym")]
    #[tokio::test]
    async fn a_mixnet_send_routes_around_the_suppressor_and_never_the_sync_indexer() {
        let (stage, mut recipient) = stage().await;
        recipient.set_destination_servers_for_tests(stage.set_over(&[
            (&stage.suppressing, SUPPRESSOR),
            (&stage.accepting, ACCEPTOR),
        ]));

        let reports = send(&mut recipient).await.unwrap();

        assert!(
            reports
                .iter()
                .all(|report| matches!(report.route, TransmitRoute::Mixnet { .. }))
        );
        let (sync, accepting, _) = stage.received().await;
        assert_eq!((sync, accepting), (0, reports.len()));
    }

    #[cfg(feature = "nym")]
    #[tokio::test]
    async fn an_all_suppressing_mixnet_draw_fails_closed() {
        let (stage, mut recipient) = stage().await;
        recipient
            .set_destination_servers_for_tests(stage.set_over(&[(&stage.suppressing, SUPPRESSOR)]));

        let refused = send(&mut recipient)
            .await
            .expect_err("a suppressed transmission surfaces");
        assert!(
            matches!(
                refused,
                LightClientError::SendError(
                    crate::lightclient::error::SendError::TransmissionError(_)
                )
            ),
            "the refusal is the typed transmission failure: {refused}"
        );
        let (sync, _, rejected) = stage.received().await;
        assert_eq!(
            sync, 0,
            "a refused draw never falls back to the sync indexer"
        );
        assert!(rejected > 0, "the suppressing Destination was contacted");
    }
}

/// The real mixnet wire, end to end through a loopback SOCKS5 relay.
#[cfg(feature = "nym")]
mod mixnet_wire_offline {
    use super::*;
    use crate::destination::health::FaultDomain;
    use crate::destination::servers::{DestinationServerSet, IndexerConfig, Location, Role, Trust};
    use crate::lightclient::LightClient;
    use crate::lightclient::error::LightClientError;
    use crate::lightclient::migrate::transmission_route::{
        MigrationWire, RoutedTransmissionClient,
    };
    use crate::lightclient::send::{TransmitReport, TransmitRoute};
    use crate::testutils::mock_indexer::Rules;
    use crate::testutils::socks5_relay::{Destination, Socks5Relay, destination_of};
    use crate::wallet::migration::transmission::{
        PartTransmissionError, TransmissionClient as _, TransmissionRoute,
    };
    use nonempty::NonEmpty;

    const ACCEPTOR_URI: &str = "https://localhost:443";
    const SUPPRESSOR_URI: &str = "https://127.0.0.1:443";
    const UNROUTED_URI: &str = "https://unrouted.example:443";
    const ACCEPTOR: &str = "acceptor.example";
    const SUPPRESSOR: &str = "suppressor.example";
    const UNROUTED: &str = "unrouted.example";
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("a static uri")
    }

    struct Stage {
        sync: MockNet,
        accepting: MockNet,
        suppressing: MockNet,
        relay: Socks5Relay,
    }

    impl Stage {
        fn set_over(&self, entries: &[(&str, &str)]) -> DestinationServerSet {
            DestinationServerSet::registry_for_tests(
                Trust::Untrusted,
                entries
                    .iter()
                    .map(|(text, operator)| (uri(text), *operator)),
            )
            .with_indexer(IndexerConfig::new(self.sync.indexer_uri()).location(Location::Remote))
        }

        fn sync_destination(&self) -> Destination {
            destination_of(&self.sync.indexer_uri())
        }
    }

    async fn relayed() -> (Socks5Relay, MockNet, MockNet) {
        let accepting = MockNet::launch_tls().await;
        let suppressing = MockNet::launch_tls().await;
        suppressing.chain.write().await.reject_all_sends = true;
        let relay = Socks5Relay::launch().await;
        relay.route(&uri(ACCEPTOR_URI), accepting.addr());
        relay.route(&uri(SUPPRESSOR_URI), suppressing.addr());
        (relay, accepting, suppressing)
    }

    async fn stage() -> (Stage, LightClient) {
        let mut sync = MockNet::launch().await;
        let (relay, accepting, suppressing) = relayed().await;
        let mut recipient = sync
            .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .await;
        let recipient_ua =
            get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
        fund_mirrored(
            &[&sync, &accepting, &suppressing],
            vec![(&recipient_ua, 100_000, None)],
            1,
        )
        .await;
        recipient.sync_and_await().await.unwrap();
        recipient.set_transmit_retry_interval(std::time::Duration::from_millis(10));
        recipient
            .switch_on_mixnet_through_for_tests(relay.addr())
            .await;
        (
            Stage {
                sync,
                accepting,
                suppressing,
                relay,
            },
            recipient,
        )
    }

    async fn send(
        recipient: &mut LightClient,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        from_inputs::quick_send_reported(
            recipient,
            vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
        )
        .await
    }

    #[tokio::test]
    async fn a_mixnet_send_crosses_the_tunnel_to_a_registry_destination() {
        let (stage, mut recipient) = stage().await;
        recipient.set_destination_servers_for_tests(
            stage.set_over(&[(ACCEPTOR_URI, ACCEPTOR), (SUPPRESSOR_URI, SUPPRESSOR)]),
        );

        let reports = send(&mut recipient).await.unwrap();

        for report in &reports {
            assert_eq!(
                report.route,
                TransmitRoute::Mixnet {
                    destination: "localhost".to_string(),
                    via_socks5: stage.relay.addr().to_string(),
                }
            );
        }
        assert_eq!(
            stage.accepting.chain.read().await.mempool_len(),
            reports.len()
        );
        assert_eq!(stage.sync.chain.read().await.mempool_len(), 0);
        let allowed = [
            destination_of(&uri(ACCEPTOR_URI)),
            destination_of(&uri(SUPPRESSOR_URI)),
        ];
        let requested = stage.relay.requested();
        assert!(!requested.is_empty());
        assert!(
            requested
                .iter()
                .all(|destination| allowed.contains(destination)),
            "the exit learned a destination outside the registry: {requested:?}"
        );
        assert!(!requested.contains(&stage.sync_destination()));
    }

    #[tokio::test]
    async fn failed_arms_are_attributed_to_the_component_that_failed() {
        let (stage, mut recipient) = stage().await;
        recipient.set_destination_servers_for_tests(
            stage.set_over(&[(SUPPRESSOR_URI, SUPPRESSOR), (UNROUTED_URI, UNROUTED)]),
        );

        let refused = send(&mut recipient)
            .await
            .expect_err("no Destination accepts");
        assert!(
            matches!(
                refused,
                LightClientError::SendError(
                    crate::lightclient::error::SendError::TransmissionError(_)
                )
            ),
            "{refused}"
        );
        let attempts = recipient.indexer_history_handle().load();
        let fault_of = |host: &str| {
            attempts
                .iter()
                .find(|attempt| attempt.host.as_str() == host)
                .unwrap_or_else(|| panic!("no attempt against {host}: {attempts:?}"))
                .fault_domain
        };
        assert_eq!(fault_of("127.0.0.1"), Some(FaultDomain::Destination));
        assert_eq!(fault_of("unrouted.example"), Some(FaultDomain::Tunnel));
        assert!(stage.suppressing.chain.read().await.rejected_sends > 0);
        assert!(
            stage
                .relay
                .requested()
                .contains(&destination_of(&uri(UNROUTED_URI))),
            "the unroutable arm asked the exit for its Destination"
        );
    }

    #[tokio::test]
    async fn a_trusted_broadcast_indexer_receives_the_mixnet_send_alone() {
        let (stage, mut recipient) = stage().await;
        recipient.set_destination_servers_for_tests(
            stage
                .set_over(&[(SUPPRESSOR_URI, SUPPRESSOR)])
                .with_indexer(
                    IndexerConfig::new(uri(ACCEPTOR_URI))
                        .role(Role::Broadcast)
                        .trust(Trust::Trusted)
                        .location(Location::Remote),
                ),
        );

        let reports = send(&mut recipient).await.unwrap();

        assert_eq!(
            stage.accepting.chain.read().await.mempool_len(),
            reports.len()
        );
        assert_eq!(stage.suppressing.chain.read().await.rejected_sends, 0);
        let acceptor = destination_of(&uri(ACCEPTOR_URI));
        assert!(
            stage
                .relay
                .requested()
                .iter()
                .all(|destination| *destination == acceptor),
            "{:?}",
            stage.relay.requested()
        );
    }

    #[tokio::test]
    async fn the_network_probe_measures_the_registry_through_the_tunnel() {
        let (stage, mut recipient) = stage().await;
        recipient.set_destination_servers_for_tests(
            stage.set_over(&[(ACCEPTOR_URI, ACCEPTOR), (SUPPRESSOR_URI, SUPPRESSOR)]),
        );

        let probes = recipient
            .probe_destinations(None, PROBE_TIMEOUT)
            .await
            .expect("a ready mixnet probes");

        let mut hosts: Vec<&str> = probes.iter().map(|probe| probe.host.as_str()).collect();
        hosts.sort_unstable();
        assert_eq!(hosts, vec!["127.0.0.1", "localhost"]);
        for probe in &probes {
            assert!(probe.leg.outcome.is_ok(), "{probe:?}");
        }
        assert!(!stage.relay.requested().contains(&stage.sync_destination()));
    }

    #[tokio::test]
    async fn a_migration_part_crosses_the_tunnel() {
        const REJECTION_CODE: i32 = -26;
        const REJECTOR_URI: &str = "https://localhost:8443";

        let (relay, accepting, suppressing) = relayed().await;
        let rejecting = MockNet::launch_tls().await;
        for net in [&accepting, &suppressing, &rejecting] {
            net.chain.write().await.rules = Rules::LAX;
        }
        rejecting.chain.write().await.answer_sends_with_error_code = Some(REJECTION_CODE);
        relay.route(&uri(REJECTOR_URI), rejecting.addr());
        let part =
            faucet_funding_transaction(vec![(&external_address(PoolType::ORCHARD), 20_000, None)])
                .await;
        let expiry = BlockHeight::from_u32(1);
        let client_for = |target: &str| {
            RoutedTransmissionClient::new(
                MigrationWire::Mixnet(crate::mixnet::MixnetConduit::over(relay.addr()).dial()),
                vec![uri(target)],
            )
        };

        let receipt = client_for(ACCEPTOR_URI)
            .submit(part.clone(), expiry)
            .await
            .expect("the acceptor takes the part");
        assert_eq!(
            receipt.route,
            TransmissionRoute::Mixnet {
                destination: "localhost".to_string(),
                via_socks5: relay.addr().to_string(),
            }
        );
        assert_eq!(accepting.chain.read().await.mempool_len(), 1);

        assert!(matches!(
            client_for(REJECTOR_URI).submit(part.clone(), expiry).await,
            Err(PartTransmissionError::Rejected(_))
        ));
        assert!(matches!(
            client_for(SUPPRESSOR_URI)
                .submit(part.clone(), expiry)
                .await,
            Err(PartTransmissionError::Transport(_))
        ));
        assert!(matches!(
            client_for(UNROUTED_URI).submit(part, expiry).await,
            Err(PartTransmissionError::Transport(_))
        ));
    }
}
