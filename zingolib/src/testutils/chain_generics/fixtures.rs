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
use crate::testutils::lightclient::get_base_address;

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
    .await;
    assert_eq!(sender.value_transfers().await.0.len(), 3);
    assert_eq!(
        sender.value_transfers().await.0[0].kind(),
        ValueTransferKind::Received
    );
    assert_eq!(
        sender.value_transfers().await.0[1].kind(),
        ValueTransferKind::Sent(SentValueTransfer::Send)
    );
    assert_eq!(
        sender.value_transfers().await.0[2].kind(),
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
            SelfSendValueTransfer::MemoToSelf
        ))
    );
    assert_eq!(recipient.value_transfers().await.0.len(), 1);
    assert_eq!(
        recipient.value_transfers().await.0[0].kind(),
        ValueTransferKind::Received
    );

    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &sender,
        vec![(&sender, PoolType::Shielded(Orchard), send_value_self, None)],
        false,
    )
    .await;
    assert_eq!(sender.value_transfers().await.0.len(), 4);
    assert_eq!(
        sender.value_transfers().await.0[3].kind(),
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Basic))
    );

    with_assertions::assure_propose_shield_bump_sync(&mut environment, &sender, false)
        .await
        .unwrap();
    assert_eq!(sender.value_transfers().await.0.len(), 5);
    assert_eq!(
        sender.value_transfers().await.0[4].kind(),
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Shield))
    );
}
/// runs a send-to-receiver and receives it in a chain-generic context
pub async fn propose_and_broadcast_value_to_pool<CC>(send_value: u64, pooltype: PoolType)
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    println!("chain set up, funding client now");

    let expected_fee = MARGINAL_FEE.into_u64()
        * match pooltype {
            // contribution_transparent = 1
            //  1 transfer
            // contribution_orchard = 2
            //  1 input
            //  1 dummy output
            Transparent => 3,
            // contribution_sapling = 2
            //  1 output
            //  1 dummy input
            // contribution_orchard = 2
            //  1 input
            //  1 dummy output
            Shielded(Sapling) => 4,
            // contribution_orchard = 2
            //  1 input
            //  1 output
            Shielded(Orchard) => 2,
        };

    let sender = environment
        .fund_client_orchard(send_value + expected_fee)
        .await;

    println!("client is ready to send");

    let recipient = environment.create_client().await;

    println!("recipient ready");

    let recorded_fee = with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &sender,
        vec![(&recipient, pooltype, send_value, None)],
        false,
    )
    .await;

    assert_eq!(expected_fee, recorded_fee);
}

/// required change should be 0
pub async fn change_required<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let primary = environment.fund_client_orchard(45_000).await;
    let secondary = environment.create_client().await;

    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            vec![
                (&secondary, Shielded(Orchard), 1, None),
                (&secondary, Shielded(Orchard), 29_999, None)
            ],
            false,
        )
        .await,
        3 * MARGINAL_FEE.into_u64()
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
        assert_eq!(
            with_assertions::propose_send_bump_sync_all_recipients(
                &mut environment,
                &primary,
                vec![
                    (&secondary, Transparent, 100_000, None),
                    (&secondary, Transparent, 4_000, None)
                ],
                false,
            )
            .await,
            MARGINAL_FEE.into_u64() * 4
        );

        assert_eq!(
            with_assertions::assure_propose_shield_bump_sync(&mut environment, &secondary, false,)
                .await,
            Ok(MARGINAL_FEE.into_u64() * 3)
        );

        assert_eq!(
            with_assertions::propose_send_bump_sync_all_recipients(
                &mut environment,
                &secondary,
                vec![(&primary, Shielded(Orchard), 50_000, None)],
                false,
            )
            .await,
            MARGINAL_FEE.into_u64() * 2
        );
    }
}

/// uses a dust input to pad another input to finish a transaction
pub async fn send_required_dust<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let primary = environment.fund_client_orchard(120_000).await;
    let secondary = environment.create_client().await;

    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            vec![
                (&secondary, Shielded(Orchard), 1, None),
                (&secondary, Shielded(Orchard), 99_999, None)
            ],
            false,
        )
        .await,
        3 * MARGINAL_FEE.into_u64()
    );

    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(&primary, Shielded(Orchard), 90_000, None)],
            false,
        )
        .await,
        2 * MARGINAL_FEE.into_u64()
    );
}

/// uses a dust input to pad another input to finish a transaction
pub async fn send_grace_dust<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let primary = environment.fund_client_orchard(120_000).await;
    let secondary = environment.create_client().await;

    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            vec![
                (&secondary, Shielded(Orchard), 1, None),
                (&secondary, Shielded(Orchard), 99_999, None)
            ],
            false,
        )
        .await,
        3 * MARGINAL_FEE.into_u64()
    );

    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(&primary, Shielded(Orchard), 30_000, None)],
            false,
        )
        .await,
        2 * MARGINAL_FEE.into_u64()
    );

    // since we used our dust as a freebie in the last send, we should only have 1
    let secondary_outputs = secondary.list_outputs().await;
    let spent_orchard_outputs: Vec<_> = secondary_outputs
        .iter()
        .filter(|o| matches!(o.pool_type(), Shielded(Orchard)))
        .filter(|o| o.is_spent_confirmed())
        .collect();
    assert_eq!(spent_orchard_outputs.len(), 1);
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
    assert_eq!(
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
        .await,
        11 * MARGINAL_FEE.into_u64()
    );

    // combine the only valid sapling note with the only valid orchard note to send
    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(&primary, Shielded(Orchard), 10_000, None),],
            false,
        )
        .await,
        4 * MARGINAL_FEE.into_u64()
    );
}

/// creates a proposal, sends it and receives it (upcoming: compares that it was executed correctly) in a chain-generic context
pub async fn send_value_to_pool<CC>(send_value: u64, pooltype: PoolType)
where
    CC: ConductChain,
{
    let multiple = match pooltype {
        PoolType::Shielded(Orchard) => 2u64,
        PoolType::Shielded(Sapling) => 4u64,
        PoolType::Transparent => 3u64,
    };
    let mut environment = CC::setup().await;

    let sender = environment
        .fund_client_orchard(send_value + multiple * (MARGINAL_FEE.into_u64()))
        .await;

    let recipient = environment.create_client().await;
    let recipient_address = get_base_address(&recipient, pooltype).await;

    from_inputs::quick_send(
        &sender,
        vec![(recipient_address.as_str(), send_value, None)],
    )
    .await
    .unwrap();

    environment.bump_chain().await;

    recipient.do_sync(false).await.unwrap();

    assert_eq!(
        recipient
            .query_sum_value(OutputQuery {
                spend_status: OutputSpendStatusQuery::only_unspent(),
                pools: OutputPoolQuery::one_pool(pooltype),
            })
            .await,
        send_value
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
    let value_from_transaction_2: u64 = 40_000;

    let transaction_1_values = (1..=number_of_notes).map(|n| n * 10_000);

    let expected_fee_for_transaction_1 = (number_of_notes + 2) * MARGINAL_FEE.into_u64();
    let expected_value_from_transaction_1: u64 = transaction_1_values.clone().sum();

    let mut environment = CC::setup().await;
    let primary = environment
        .fund_client_orchard(expected_fee_for_transaction_1 + expected_value_from_transaction_1)
        .await;
    let secondary = environment.create_client().await;

    // Send number_of_notes transfers in increasing 10_000 zat increments
    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &primary,
            transaction_1_values
                .map(|value| (&secondary, Shielded(Sapling), value, None))
                .collect(),
            false,
        )
        .await,
        expected_fee_for_transaction_1
    );

    assert_eq!(
        secondary
            .query_sum_value(OutputQuery {
                spend_status: OutputSpendStatusQuery::only_unspent(),
                pools: OutputPoolQuery::one_pool(Shielded(Sapling)),
            })
            .await,
        expected_value_from_transaction_1
    );

    let expected_orchard_contribution_for_transaction_2 = 2;

    // calculate what will be spent
    let mut expected_highest_unselected: i64 = 10_000 * number_of_notes as i64;
    let mut expected_inputs_for_transaction_2 = 0;
    let mut max_unselected_value_for_transaction_2: i64 =
        (value_from_transaction_2 + expected_orchard_contribution_for_transaction_2) as i64;
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
    assert_eq!(
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &secondary,
            vec![(&primary, Shielded(Orchard), value_from_transaction_2, None)],
            false,
        )
        .await,
        expected_fee_for_transaction_2
    );

    let expected_debit_from_transaction_2 =
        expected_fee_for_transaction_2 + value_from_transaction_2;
    assert_eq!(
        secondary
            .query_sum_value(OutputQuery {
                spend_status: OutputSpendStatusQuery::only_unspent(),
                pools: OutputPoolQuery::shielded(),
            })
            .await,
        expected_value_from_transaction_1 - expected_debit_from_transaction_2
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

/// the simplest test that sends from a specific shielded pool to another specific pool. also known as simpool.
pub async fn shpool_to_pool<CC>(shpool: ShieldedProtocol, pool: PoolType, make_change: u64)
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;

    let primary = environment.fund_client_orchard(1_000_000).await;
    let secondary = environment.create_client().await;
    with_assertions::propose_send_bump_sync_all_recipients(
        &mut environment,
        &primary,
        vec![(&secondary, Shielded(shpool), 100_000 + make_change, None)],
        false,
    )
    .await;

    let tertiary = environment.create_client().await;
    let expected_fee = fee_tables::one_to_one(Some(shpool), pool, true);

    let ref_primary: Arc<LightClient> = Arc::new(primary);
    let ref_secondary: Arc<LightClient> = Arc::new(secondary);
    let ref_tertiary: Arc<LightClient> = Arc::new(tertiary);

    // mempool monitor
    let check_mempool = !cfg!(feature = "ci");
    if check_mempool {
        for lightclient in [&ref_primary, &ref_secondary, &ref_tertiary] {
            assert!(LightClient::start_mempool_monitor(lightclient.clone()).is_ok());
            dbg!("mm started");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }

    assert_eq!(
        expected_fee,
        with_assertions::propose_send_bump_sync_all_recipients(
            &mut environment,
            &ref_secondary,
            vec![(&ref_tertiary, pool, 100_000 - expected_fee, None)],
            check_mempool,
        )
        .await
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
    .await;

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
