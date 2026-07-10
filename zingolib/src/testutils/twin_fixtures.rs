//! Environment-generic bodies for the live/offline twin tests.
//!
//! Each twin pair (an offline test in
//! [`crate::lightclient::mock_chain_tests`] and its live original in
//! libtonode's `unit_test_twins`) runs one fixture from this module
//! against its own [`TwinChain`] implementation, so the protection is
//! written exactly once. The genuine environment divergences —
//! funding-confirmation height and the faucet's note-pool economics —
//! are explicit trait hooks rather than forked test bodies; everything
//! a fixture asserts about the *recipient wallet* is environment
//! independent.

use zcash_primitives::transaction::fees::zip317::{MARGINAL_FEE, MINIMUM_FEE};
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingo_test_vectors::TEST_TXID;

use crate::check_client_balances;
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::testutils::lightclient::{from_inputs, get_base_address, get_fees_paid_by_client};
use crate::testutils::{assert_transaction_summary_equality, assert_transaction_summary_exists};
use crate::utils::conversion::txid_from_hex_encoded_str;
use crate::wallet::error::ProposeSendError;
use crate::wallet::output::SpendStatus;
use crate::wallet::summary::data::{
    BasicNoteSummary, OutgoingNoteSummary, Scope as SummaryScope, SendType, SentValueTransfer,
    TransactionKind, TransactionSummary, ValueTransferKind,
};

/// The environment a twin fixture runs against: the mock indexer for the
/// offline twin, LocalNet for the live original.
///
/// Both sides present the same shape — a real sending faucet on the
/// abandon-art identity and a recipient on the hospital-museum identity —
/// so the fixtures' pinned address literals hold in either environment.
#[allow(async_fn_in_trait)]
pub trait TwinChain: Sized {
    /// A chain with a spendable faucet and an unfunded recipient.
    async fn setup_faucet_recipient() -> (Self, LightClient, LightClient);

    /// As [`Self::setup_faucet_recipient`], with `initial` zatoshis
    /// already confirmed in the recipient's orchard pool.
    async fn setup_funded_recipient(initial: u64) -> (Self, LightClient, LightClient);

    /// Advances the chain one block, confirming mempool contents.
    async fn bump(&mut self);

    /// Syncs `client` to the chain tip.
    async fn sync(&self, client: &mut LightClient);

    /// [`Self::bump`], then [`Self::sync`] for `client`.
    async fn bump_and_sync(&mut self, client: &mut LightClient) {
        self.bump().await;
        self.sync(client).await;
    }

    /// The height at which [`Self::setup_funded_recipient`]'s funding
    /// transaction confirmed — the base the summary-pinning fixture
    /// offsets from.
    fn funded_setup_height(&self) -> u32;

    /// The faucet's ZIP-317 fee for its *second* funding wave. Live, the
    /// faucet's note pool is fragmented by earlier waves and
    /// smallest-first selection makes the send four logical actions
    /// (20_000); the mock faucet spends one large change note (10_000).
    /// The fee belongs to the faucet's economics, not to the recipient
    /// behavior the fixture protects.
    fn second_wave_faucet_fee(&self) -> u64;

    /// Post-step-1 diagnostics hook for the pool-promotion ladder
    /// (surfaces the mock's transparent-address request ledger on
    /// zingolabs/zingolib#2447-shaped failures). Default: nothing.
    async fn step1_diagnostics(&self, _client: &LightClient) {}
}

/// A zero-value receipt surfaces as exactly one Received{0, Orchard}
/// value transfer and must not perturb spendable arithmetic across a
/// subsequent send.
pub async fn zero_value_receipts<TC: TwinChain>() {
    let (mut env, mut faucet, mut recipient) = TC::setup_funded_recipient(100_000).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let faucet_ua = get_base_address(&faucet, PoolType::Shielded(ShieldedPool::Orchard)).await;

    from_inputs::quick_send(&mut faucet, vec![(recipient_ua.as_str(), 0, None)])
        .await
        .unwrap();
    env.bump_and_sync(&mut recipient).await;

    from_inputs::quick_send(&mut recipient, vec![(faucet_ua.as_str(), 1_000, None)])
        .await
        .unwrap();
    env.bump_and_sync(&mut recipient).await;

    // The zero-value receipt must not perturb spendable arithmetic: the
    // recipient holds the 100_000 funding note less the 1_000 payment and
    // its 10_000 ZIP-317 fee (one orchard spend, two logical actions).
    check_client_balances!(recipient, o: 89_000 s: 0 t: 0);

    let value_transfers = recipient.value_transfers(true).await.unwrap();
    // The funding receipt.
    assert!(
        value_transfers
            .iter()
            .any(|vt| vt.kind == ValueTransferKind::Received && vt.value == 100_000)
    );
    // Pinned by observation rather than specification: the zero-value
    // receipt surfaces as a single Received transfer of zero value in
    // the orchard pool, carried without corruption.
    assert_eq!(
        value_transfers
            .iter()
            .filter(|vt| vt.kind == ValueTransferKind::Received
                && vt.value == 0
                && vt.pool_received.as_deref() == Some("Orchard"))
            .count(),
        1
    );
    // The subsequent spend proceeds unimpeded by the zero-value note.
    assert!(value_transfers.iter().any(|vt| {
        vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send)
            && vt.value == 1_000
            && vt.transaction_fee == Some(10_000)
    }));
    assert_eq!(value_transfers.iter().count(), 3);
}

/// A two-output cross-pool self-send costs the exact composite ZIP-317
/// fee — 5_000 for the transparent output, 10_000 for the orchard side,
/// 10_000 for the sapling output — and every pool balance lands where
/// the live original pinned it.
pub async fn list_value_transfers_check_fees<TC: TwinChain>() {
    let (mut env, mut faucet, mut client) = TC::setup_faucet_recipient().await;
    let client_ua = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let client_taddr = get_base_address(&client, PoolType::Transparent).await;
    let client_sapling = get_base_address(&client, PoolType::Shielded(ShieldedPool::Sapling)).await;

    from_inputs::quick_send(&mut faucet, vec![(client_ua.as_str(), 100_000, None)])
        .await
        .unwrap();
    env.bump_and_sync(&mut client).await;
    check_client_balances!(client, o: 100_000 s: 0 t: 0);

    from_inputs::quick_send(
        &mut client,
        vec![
            (client_taddr.as_str(), 30_000, None),
            (client_sapling.as_str(), 30_000, None),
        ],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut client).await;

    // 100_000 − 30_000 − 30_000 − 25_000 fee = 15_000 orchard change.
    check_client_balances!(client, o: 15_000 s: 30_000 t: 30_000);
}

/// Mixed self-sends to the wallet's own transparent, sapling, and
/// orchard addresses — plus an incoming mixed send mined in the same
/// block — must each surface as ONE transaction, so every
/// transaction-summary txid is unique.
pub async fn self_send_to_t_displays_as_one_transaction<TC: TwinChain>() {
    let (mut env, mut faucet, mut recipient) = TC::setup_faucet_recipient().await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let recipient_taddr = get_base_address(&recipient, PoolType::Transparent).await;
    let recipient_zaddr =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Sapling)).await;

    from_inputs::quick_send(&mut faucet, vec![(recipient_ua.as_str(), 80_000, None)])
        .await
        .unwrap();
    env.bump_and_sync(&mut recipient).await;

    let sent_to_taddr_value = 5_000;
    let sent_to_zaddr_value = 11_000;
    let sent_to_self_orchard_value = 1_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;

    // The recipient's own mixed self-send and an incoming mixed send,
    // mined into the same block.
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
    env.sync(&mut faucet).await;
    from_inputs::quick_send(
        &mut faucet,
        vec![
            (recipient_taddr.as_str(), sent_to_taddr_value, None),
            (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo2")),
            (
                recipient_ua.as_str(),
                sent_to_self_orchard_value,
                Some("bar2"),
            ),
        ],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;

    let txids = recipient
        .transaction_summaries(false)
        .await
        .unwrap()
        .txids();
    let unique: std::collections::HashSet<_> = txids.iter().collect();
    assert_eq!(
        unique.len(),
        txids.len(),
        "every self-send surfaces as exactly one transaction"
    );
}

/// Full transaction-summary pinning across funding waves, cross-pool
/// sends, and the Transmitted-to-Confirmed transition of an unmined
/// send. Heights are pinned relative to
/// [`TwinChain::funded_setup_height`]; the second funding wave's fee is
/// [`TwinChain::second_wave_faucet_fee`] (faucet economics, documented
/// there).
pub async fn send_to_transparent_and_sapling_maintain_balance<TC: TwinChain>() {
    use pepper_sync::wallet::OrchardNote;

    let recipient_initial_funds = 100_000_000;
    let (mut env, mut faucet, mut recipient) =
        TC::setup_funded_recipient(recipient_initial_funds).await;
    let base = env.funded_setup_height();
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    // The external destinations: the faucet's sapling address and first
    // taddr — the abandon-art derivations in both environments, which is
    // what pins the recipient/recipient_unified_address literals below.
    let external_sapling =
        get_base_address(&faucet, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let external_taddr = get_base_address(&faucet, PoolType::Transparent).await;

    let placeholder_txid = txid_from_hex_encoded_str(TEST_TXID).unwrap();
    let summary_orchard_receipt = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base)),
        blockheight: BlockHeight::from_u32(base),
        kind: TransactionKind::Received,
        value: recipient_initial_funds,
        fee: Some(10_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            recipient_initial_funds,
            SpendStatus::Spent(placeholder_txid),
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
    };

    // Send to external sapling, mined at base + 1.
    let first_send_to_sapling = 20_000;
    from_inputs::quick_send(
        &mut recipient,
        vec![(external_sapling.as_str(), first_send_to_sapling, None)],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;
    let summary_external_sapling = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base + 1)),
        blockheight: BlockHeight::from_u32(base + 1),
        kind: TransactionKind::Sent(SendType::Send),
        value: first_send_to_sapling,
        fee: Some(20_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            99_960_000,
            SpendStatus::TransmittedSpent(placeholder_txid),
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
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
        outgoing_ironwood_notes: vec![],
    };

    // Send to external transparent, left in the mempool: Transmitted,
    // targeting base + 2.
    let first_send_to_transparent = 20_000;
    let summary_external_transparent = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Transmitted(BlockHeight::from_u32(base + 2)),
        blockheight: BlockHeight::from_u32(base + 2),
        kind: TransactionKind::Sent(SendType::Send),
        value: first_send_to_transparent,
        fee: Some(15_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            99_925_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
    };
    from_inputs::quick_send(
        &mut recipient,
        vec![(external_taddr.as_str(), first_send_to_transparent, None)],
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

    // Mid-flight balances: everything sits in the unmined send's change,
    // unconfirmed; nothing is confirmed-spendable.
    let expected_funds = recipient_initial_funds
        - first_send_to_sapling
        - (4 * u64::from(MARGINAL_FEE))
        - first_send_to_transparent
        - (3 * u64::from(MARGINAL_FEE));
    {
        let recipient_wallet = recipient.wallet();
        let recipient_wallet = recipient_wallet.read().await;
        assert_eq!(
            recipient_wallet
                .unconfirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                .unwrap(),
            expected_funds.try_into().unwrap()
        );
        assert_eq!(
            recipient_wallet
                .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                .unwrap(),
            0.try_into().unwrap()
        );
    }

    // The pending transparent send confirms at base + 2; the faucet syncs
    // so it can fund the second wave.
    env.bump_and_sync(&mut faucet).await;
    env.sync(&mut recipient).await;

    // Second funding wave, with a memo, at base + 3.
    let recipient_second_funding = 1_000_000;
    let summary_orchard_receipt_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base + 3)),
        blockheight: BlockHeight::from_u32(base + 3),
        kind: TransactionKind::Received,
        value: recipient_second_funding,
        fee: Some(env.second_wave_faucet_fee()),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            recipient_second_funding,
            SpendStatus::Spent(placeholder_txid),
            0,
            Some("Second wave incoming".to_string()),
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
    };
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            recipient_ua.as_str(),
            recipient_second_funding,
            Some("Second wave incoming"),
        )],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;

    // Second wave of external sends: transparent and sapling mined into
    // the same block, base + 4.
    let second_send_to_transparent = 20_000;
    let second_send_to_sapling = 20_000;
    let summary_external_transparent_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base + 4)),
        blockheight: BlockHeight::from_u32(base + 4),
        kind: TransactionKind::Sent(SendType::Send),
        value: second_send_to_transparent,
        fee: Some(15_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            965_000,
            SpendStatus::Spent(placeholder_txid),
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
    };
    let summary_external_sapling_2 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base + 4)),
        blockheight: BlockHeight::from_u32(base + 4),
        kind: TransactionKind::Sent(SendType::Send),
        value: second_send_to_sapling,
        fee: Some(20_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            99_885_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
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
        outgoing_ironwood_notes: vec![],
    };
    from_inputs::quick_send(
        &mut recipient,
        vec![(external_taddr.as_str(), second_send_to_transparent, None)],
    )
    .await
    .unwrap();
    from_inputs::quick_send(
        &mut recipient,
        vec![(external_sapling.as_str(), second_send_to_sapling, None)],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;

    // Third external transparent, mined at base + 5.
    let external_transparent_3 = 20_000;
    let summary_external_transparent_3 = TransactionSummary {
        txid: placeholder_txid,
        datetime: 0,
        status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(base + 5)),
        blockheight: BlockHeight::from_u32(base + 5),
        kind: TransactionKind::Sent(SendType::Send),
        value: external_transparent_3,
        fee: Some(15_000),
        zec_price: None,
        orchard_notes: vec![BasicNoteSummary::from_parts(
            930_000,
            SpendStatus::Unspent,
            0,
            None,
        )],
        sapling_notes: vec![],
        transparent_coins: vec![],
        ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
    };
    from_inputs::quick_send(
        &mut recipient,
        vec![(external_taddr.as_str(), external_transparent_3, None)],
    )
    .await
    .unwrap();
    env.bump_and_sync(&mut recipient).await;

    assert_transaction_summary_equality(
        &recipient.transaction_summaries(false).await.unwrap().0[3],
        &summary_orchard_receipt_2,
    );
    // Two sends share base + 4, so summary order within the block is not
    // pinned; assert existence instead.
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
        - (5 * u64::from(MINIMUM_FEE));
    assert_eq!(
        recipient
            .wallet()
            .read()
            .await
            .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
            .unwrap(),
        second_wave_expected_funds.try_into().unwrap(),
    );
}

/// The full pool-promotion ledger — every funding source and self-send
/// combination, two shields, exact per-step balances, and the cumulative
/// confirmed-fee total.
pub async fn from_t_z_o_tz_to_zo_tzo_to_orchard<TC: TwinChain>() {
    let (mut env, mut faucet, mut client) = TC::setup_faucet_recipient().await;
    let pmc_unified = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let pmc_taddr = get_base_address(&client, PoolType::Transparent).await;
    let pmc_sapling = get_base_address(&client, PoolType::Shielded(ShieldedPool::Sapling)).await;

    macro_rules! bump_and_check {
        (o: $o:tt s: $s:tt t: $t:tt) => {
            env.bump_and_sync(&mut client).await;
            check_client_balances!(client, o:$o s:$s t:$t);
        };
    }

    let mut total_expected_fee = 0;
    // 1 receive 50_000 transparent.
    from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 50_000, None)])
        .await
        .unwrap();
    env.bump_and_sync(&mut client).await;
    env.step1_diagnostics(&client).await;
    check_client_balances!(client, o: 0 s: 0 t: 50_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 2 shield 50_000 transparent to orchard: 15_000 (1 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 35_000 s: 0 t: 0);
    total_expected_fee += 15_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 3 receive 50_000 sapling
    env.sync(&mut faucet).await;
    from_inputs::quick_send(&mut faucet, vec![(&pmc_sapling, 50_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 35_000 s: 50_000 t: 0);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 4 migrate sapling to orchard: 20_000 (2 sapling, 2 orchard)
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 65_000 s: 0 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 5 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 55_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 55_000 s: 0 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 6 orchard to own transparent and sapling: 25_000
    from_inputs::quick_send(
        &mut client,
        vec![(&pmc_taddr, 10_000, None), (&pmc_sapling, 10_000, None)],
    )
    .await
    .unwrap();
    bump_and_check!(o: 10_000 s: 10_000 t: 10_000);
    total_expected_fee += 25_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 7 receive 500_000 transparent
    env.sync(&mut faucet).await;
    from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 500_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 10_000 s: 10_000 t: 510_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 8 shield both coins: 20_000 (2 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 500_000 s: 10_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 9 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 490_000 s: 10_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 10 orchard + sapling demoted to own transparent: 30_000
    from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 470_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 0 t: 470_000);
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
    bump_and_check!(o: 0 s: 0 t: 470_000);
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
    bump_and_check!(o: 0 s: 0 t: 470_000);
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 12 shield: 15_000 (1 t-in, 2 orchard)
    client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
    bump_and_check!(o: 455_000 s: 0 t: 0);
    total_expected_fee += 15_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 13 orchard to own sapling: 20_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 10_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 425_000 s: 10_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 14 orchard self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_unified, 20_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 415_000 s: 10_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 15 orchard + sapling to own sapling: 20_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 405_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 405_000 t: 0);
    total_expected_fee += 20_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);

    // 16 sapling self-send: 10_000
    from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 380_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 395_000 t: 0);
    total_expected_fee += 10_000;
    assert_eq!(get_fees_paid_by_client(&client).await, total_expected_fee);
}
