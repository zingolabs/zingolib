//! these functions are each meant to be 'test-in-a-box'
//! simply plug in a mock server as a chain conductor and provide some values
use std::sync::Arc;

use zcash_client_backend::PoolType;
use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use crate::lightclient::LightClient;
use crate::wallet::data::summaries::SelfSendValueTransfer;
use crate::wallet::data::summaries::SentValueTransfer;
use crate::wallet::notes::query::OutputSpendStatusQuery;
use crate::wallet::notes::{query::OutputPoolQuery, OutputInterface};
use crate::wallet::{data::summaries::ValueTransferKind, notes::query::OutputQuery};

use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::chain_generics::with_assertions;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::from_inputs;

/// Fixture for testing various vt transactions
pub async fn create_various_value_transfers<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let sender = environment.fund_client_orchard(250_000).await;
    let send_value_for_recipient = 23_000;
    let send_value_self = 17_000;

    println!("client is ready to send");

    let recipient = environment.create_client().await;
    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &sender,
        vec![
            (
                &recipient,
                PoolType::Shielded(Orchard),
                send_value_for_recipient,
                Some("Orchard sender to recipient"),
            ),
            (
                &sender,
                PoolType::Shielded(Sapling),
                send_value_self,
                Some("Orchard sender to self"),
            ),
            (&sender, PoolType::Transparent, send_value_self, None),
        ],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.sorted_value_transfers(true).await.len(), 3);

    assert!(sender
        .sorted_value_transfers(false)
        .await
        .iter()
        .any(|vt| { vt.kind() == ValueTransferKind::Received }));

    assert!(sender
        .sorted_value_transfers(false)
        .await
        .iter()
        .any(|vt| { vt.kind() == ValueTransferKind::Sent(SentValueTransfer::Send) }));

    assert!(sender.sorted_value_transfers(false).await.iter().any(|vt| {
        vt.kind()
            == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                SelfSendValueTransfer::MemoToSelf,
            ))
    }));

    assert_eq!(recipient.sorted_value_transfers(true).await.len(), 1);

    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &sender,
        vec![(&sender, PoolType::Shielded(Orchard), send_value_self, None)],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.sorted_value_transfers(true).await.len(), 4);
    assert_eq!(
        sender.sorted_value_transfers(true).await[0].kind(),
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Basic))
    );

    with_assertions::assure_propose_shield_bump_sync(&mut environment, &sender, false)
        .await
        .unwrap();
    assert_eq!(sender.sorted_value_transfers(true).await.len(), 5);
    assert_eq!(
        sender.sorted_value_transfers(true).await[0].kind(),
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Shield))
    );
}

/// sends back and forth several times, including sends to transparent
pub async fn send_shield_cycle<CC>(n: u64)
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let primary_fund = 1_000_000;
    let primary = environment.fund_client_orchard(primary_fund).await;

    let secondary = environment.create_client().await;

    for _ in 0..n {
        let (recorded_fee, recorded_value, recorded_change) =
            with_assertions::propose_send_bump_sync_all_recipients(
                &mut environment,
                &primary,
                vec![
                    (&secondary, Transparent, 100_000, None),
                    (&secondary, Transparent, 4_000, None),
                ],
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            (recorded_fee, recorded_value, recorded_change),
            (MARGINAL_FEE.into_u64() * 4, recorded_value, recorded_change)
        );

        let (recorded_fee, recorded_value) =
            with_assertions::assure_propose_shield_bump_sync(&mut environment, &secondary, false)
                .await
                .unwrap();
        assert_eq!(
            (recorded_fee, recorded_value),
            (MARGINAL_FEE.into_u64() * 3, 100_000 - recorded_fee)
        );

        let (recorded_fee, recorded_value, recorded_change) =
            with_assertions::propose_send_bump_sync_all_recipients(
                &mut environment,
                &secondary,
                vec![(&primary, Shielded(Orchard), 50_000, None)],
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            (recorded_fee, recorded_value, recorded_change),
            (MARGINAL_FEE.into_u64() * 2, 50_000, recorded_change)
        );
    }
}

/// overlooks a bunch of dust inputs to find a pair of inputs marginally big enough to send
pub async fn ignore_dust_inputs<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    let primary = environment.fund_client_orchard(120_000).await;
    let secondary = environment.create_client().await;

    // send a bunch of dust
    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            vec![
                (&secondary, Shielded(Sapling), 1_000, None),
                (&secondary, Shielded(Sapling), 1_000, None),
                (&secondary, Shielded(Sapling), 1_000, None),
                (&secondary, Shielded(Sapling), 1_000, None),
                (&secondary, Shielded(Sapling), 15_000, None),
                (&secondary, Shielded(Orchard), 1_000, None),
                (&secondary, Shielded(Orchard), 1_000, None),
                (&secondary, Shielded(Orchard), 1_000, None),
                (&secondary, Shielded(Orchard), 1_000, None),
                (&secondary, Shielded(Orchard), 15_000, None),
            ],
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (
            11 * MARGINAL_FEE.into_u64(),
            recorded_value,
            recorded_change
        )
    );

    // combine the only valid sapling note with the only valid orchard note to send
    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(&primary, Shielded(Orchard), 10_000, None)],
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (4 * MARGINAL_FEE.into_u64(), 10_000, recorded_change)
    );
}

/// In order to fund a transaction multiple notes may be selected and consumed.
/// The algorithm selects the smallest covering note(s).
pub async fn note_selection_order<CC>()
where
    CC: ConductChain,
{
    // toDo: proptest different values for these first two variables
    let number_of_notes = 4;
    let expected_value_from_transaction_2: u64 = 40_000;

    let transaction_1_values = (1..=number_of_notes).map(|n| n * 10_000);

    let expected_fee_for_transaction_1 = (number_of_notes + 2) * MARGINAL_FEE.into_u64();
    let expected_value_from_transaction_1: u64 = transaction_1_values.clone().sum();

    let mut environment = CC::setup().await;
    let primary = environment
        .fund_client_orchard(expected_fee_for_transaction_1 + expected_value_from_transaction_1)
        .await;
    let secondary = environment.create_client().await;

    // Send number_of_notes transfers in increasing 10_000 zat increments
    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            transaction_1_values
                .map(|value| (&secondary, Shielded(Sapling), value, None))
                .collect(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (
            expected_fee_for_transaction_1,
            recorded_value,
            recorded_change
        )
    );

    let expected_orchard_contribution_for_transaction_2 = 2;

    // calculate what will be spent
    let mut expected_highest_unselected: i64 = 10_000 * number_of_notes as i64;
    let mut expected_inputs_for_transaction_2 = 0;
    let mut max_unselected_value_for_transaction_2: i64 = (expected_value_from_transaction_2
        + expected_orchard_contribution_for_transaction_2)
        as i64;
    loop {
        // add an input
        expected_inputs_for_transaction_2 += 1;
        max_unselected_value_for_transaction_2 += MARGINAL_FEE.into_u64() as i64;
        max_unselected_value_for_transaction_2 -= expected_highest_unselected;
        expected_highest_unselected -= 10_000;

        if max_unselected_value_for_transaction_2 <= 0 {
            // met target
            break;
        }
        if expected_highest_unselected <= 0 {
            // did not meet target. expect error on send
            break;
        }
    }
    let expected_fee_for_transaction_2 = (expected_inputs_for_transaction_2
        + expected_orchard_contribution_for_transaction_2)
        * MARGINAL_FEE.into_u64();

    // the second client selects notes to cover the transaction.
    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(
                &primary,
                Shielded(Orchard),
                expected_value_from_transaction_2,
                None,
            )],
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (
            expected_fee_for_transaction_2,
            expected_value_from_transaction_2,
            0
        )
    );

    let received_change_from_transaction_2 = secondary
        .query_sum_value(OutputQuery {
            spend_status: OutputSpendStatusQuery::only_unspent(),
            pools: OutputPoolQuery::one_pool(Shielded(Orchard)),
        })
        .await;
    // if 10_000 or more change, would have used a smaller note
    assert!(received_change_from_transaction_2 < 10_000);

    let all_outputs = secondary.list_outputs().await;
    let spent_sapling_outputs: Vec<_> = all_outputs
        .iter()
        .filter(|o| matches!(o.pool_type(), Shielded(Sapling)))
        .filter(|o| o.is_spent_confirmed())
        .collect();
    assert_eq!(
        spent_sapling_outputs.len(),
        expected_inputs_for_transaction_2 as usize
    );
}

/// the simplest test that sends from a specific shielded pool to another specific pool. error variant.
pub async fn shpool_to_pool_insufficient_error<CC>(
    shpool: ShieldedProtocol,
    pool: PoolType,
    underflow_amount: u64,
) where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    let primary = environment.fund_client_orchard(1_000_000).await;
    let secondary = environment.create_client().await;

    let expected_fee = fee_tables::one_to_one(Some(shpool), pool, true);
    let secondary_fund = 100_000 + expected_fee - underflow_amount;
    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &primary,
        vec![(&secondary, Shielded(shpool), secondary_fund, None)],
        false,
    )
    .await
    .unwrap();

    let tertiary = environment.create_client().await;

    let ref_secondary: Arc<LightClient> = Arc::new(secondary);
    let ref_tertiary: Arc<LightClient> = Arc::new(tertiary);

    let tertiary_fund = 100_000;
    assert_eq!(
        from_inputs::propose(
            &ref_secondary,
            vec![(
                ref_tertiary
                    .wallet
                    .get_first_address(pool)
                    .unwrap()
                    .as_str(),
                tertiary_fund,
                None,
            )],
        )
        .await
        .unwrap_err()
        .to_string(),
        format!(
            "Insufficient balance (have {}, need {} including fee)",
            secondary_fund,
            tertiary_fund + expected_fee
        )
    );
}

/// the simplest test that sends from a specific shielded pool to another specific pool. also known as simpool.
pub async fn to_pool_unfunded_error<CC>(pool: PoolType, try_amount: u64)
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    let secondary = environment.create_client().await;
    let tertiary = environment.create_client().await;

    let ref_secondary: Arc<LightClient> = Arc::new(secondary);
    let ref_tertiary: Arc<LightClient> = Arc::new(tertiary);

    ref_secondary.do_sync(false).await.unwrap();

    let expected_fee = fee_tables::one_to_one(None, pool, true);

    assert_eq!(
        from_inputs::propose(
            &ref_secondary,
            vec![(
                ref_tertiary
                    .wallet
                    .get_first_address(pool)
                    .unwrap()
                    .as_str(),
                try_amount,
                None,
            )],
        )
        .await
        .unwrap_err()
        .to_string(),
        format!(
            "Insufficient balance (have {}, need {} including fee)",
            0,
            try_amount + expected_fee
        )
    );
}

/// the simplest test that sends from a specific shielded pool to another specific pool. also known as simpool.
pub async fn single_sufficient_send<CC>(
    shpool: ShieldedProtocol,
    pool: PoolType,
    receiver_value: u64,
    change: u64,
    test_mempool: bool,
) where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    let primary = environment.fund_client_orchard(1_000_000).await;
    let secondary = environment.create_client().await;
    let tertiary = environment.create_client().await;
    let ref_primary: Arc<LightClient> = Arc::new(primary);
    let ref_secondary: Arc<LightClient> = Arc::new(secondary);
    let ref_tertiary: Arc<LightClient> = Arc::new(tertiary);

    // mempool monitor
    if test_mempool {
        for lightclient in [&ref_primary, &ref_secondary, &ref_tertiary] {
            assert!(LightClient::start_mempool_monitor(lightclient.clone()).is_ok());
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }

    let expected_fee = fee_tables::one_to_one(Some(shpool), pool, true);

    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &ref_primary,
        vec![(
            &ref_secondary,
            Shielded(shpool),
            receiver_value + change + expected_fee,
            None,
        )],
        test_mempool,
    )
    .await
    .unwrap();

    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &ref_secondary,
            vec![(&ref_tertiary, pool, receiver_value, None)],
            test_mempool,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (expected_fee, receiver_value, change)
    );
}
