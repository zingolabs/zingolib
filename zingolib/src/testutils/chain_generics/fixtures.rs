//! these functions are each meant to be 'test-in-a-box'
//! simply plug in a mock server as a chain conductor and provide some values

use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
};
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::chain_generics::with_assertions;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::timed;
use crate::testutils::timestamped_test_log;

/// Fixture for testing various vt transactions
pub async fn create_various_value_transfers<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let mut sender = environment.fund_client_orchard(250_000).await;
    let sender_orchard_addr =
        get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let sender_sapling_addr =
        get_base_address(&sender, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let sender_taddr = get_base_address(&sender, PoolType::Transparent).await;
    let send_value_for_recipient = 23_000;
    let send_value_self = 17_000;

    tracing::info!("client is ready to send");

    let mut recipient = environment.create_client().await;
    tracing::debug!("TEST 1");
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut sender,
        vec![
            (
                &get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await,
                send_value_for_recipient,
                Some("Orchard sender to recipient"),
            ),
            (
                &sender_sapling_addr,
                send_value_self,
                Some("Orchard sender to self"),
            ),
            (&sender_taddr, send_value_self, None),
        ],
        vec![&mut recipient],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 3);

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| { vt.kind == ValueTransferKind::Received })
    );

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| { vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send) })
    );

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| {
                vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::MemoToSelf,
                    ))
            })
    );

    assert_eq!(recipient.value_transfers(true).await.unwrap().len(), 1);

    tracing::debug!("TEST 2");
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut sender,
        vec![(&sender_orchard_addr, send_value_self, None)],
        vec![],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 4);
    assert_eq!(
        sender.value_transfers(true).await.unwrap()[0].kind,
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Basic))
    );

    with_assertions::assure_propose_shield_bump_sync(&mut environment, &mut sender, false)
        .await
        .unwrap();
    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 5);
    assert_eq!(
        sender.value_transfers(true).await.unwrap()[0].kind,
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Shield))
    );
}

/// sends back and forth several times, including sends to transparent
pub async fn send_shield_cycle<CC>(n: u64)
where
    CC: ConductChain,
{
    let (mut environment, ()) = tokio::join!(
        timed("send_shield_cycle::setup", CC::setup()),
        timed(
            "send_shield_cycle::warm_orchard_proving_key",
            crate::testutils::warm_orchard_proving_key(),
        ),
    );
    let primary_fund = 1_000_000;
    let mut primary = timed(
        "send_shield_cycle::fund_client_orchard",
        environment.fund_client_orchard(primary_fund),
    )
    .await;

    let mut secondary = timed(
        "send_shield_cycle::create_secondary_client",
        environment.create_client(),
    )
    .await;
    let secondary_taddr = timed(
        "send_shield_cycle::get_secondary_taddr",
        get_base_address(&secondary, PoolType::Transparent),
    )
    .await;

    for cycle in 0..n {
        timed(
            format!("send_shield_cycle::cycle[{cycle}]").as_str(),
            async {
                let (recorded_fee, recorded_value, recorded_change) = timed(
                    format!("send_shield_cycle::cycle[{cycle}]::send_to_transparent").as_str(),
                    with_assertions::assure_propose_send_bump_sync_all_recipients(
                        &mut environment,
                        &mut primary,
                        vec![
                            (&secondary_taddr, 100_000, None),
                            (&secondary_taddr, 4_000, None),
                        ],
                        vec![&mut secondary],
                        false,
                    ),
                )
                .await
                .unwrap();
                assert_eq!(
                    (recorded_fee, recorded_value, recorded_change),
                    (
                        Option::unwrap(MARGINAL_FEE * 4_u64),
                        recorded_value,
                        recorded_change
                    )
                );

                let (recorded_fee, recorded_value) = timed(
                    format!("send_shield_cycle::cycle[{cycle}]::shield").as_str(),
                    with_assertions::assure_propose_shield_bump_sync(
                        &mut environment,
                        &mut secondary,
                        false,
                    ),
                )
                .await
                .unwrap();
                assert_eq!(
                    (recorded_fee, recorded_value),
                    (
                        Option::unwrap(MARGINAL_FEE * 3_u64),
                        Option::unwrap(Zatoshis::from_u64(100_000).unwrap() - recorded_fee)
                    )
                );

                let primary_orchard_addr = timed(
                    format!("send_shield_cycle::cycle[{cycle}]::get_primary_orchard_addr").as_str(),
                    get_base_address(&primary, PoolType::Shielded(ShieldedPool::Orchard)),
                )
                .await;
                let (recorded_fee, recorded_value, recorded_change) = timed(
                    format!("send_shield_cycle::cycle[{cycle}]::send_back_to_orchard").as_str(),
                    with_assertions::assure_propose_send_bump_sync_all_recipients(
                        &mut environment,
                        &mut secondary,
                        vec![(&primary_orchard_addr, 50_000, None)],
                        vec![&mut primary],
                        false,
                    ),
                )
                .await
                .unwrap();
                assert_eq!(
                    (recorded_fee, recorded_value, recorded_change),
                    (
                        Option::unwrap(MARGINAL_FEE * 2_u64),
                        Zatoshis::from_u64(50_000).unwrap(),
                        recorded_change
                    )
                );
            },
        )
        .await;
    }
}

/// the simplest test that sends from a specific shielded pool to another specific pool. also known as simpool.
pub async fn any_source_sends_to_any_receiver<CC>(
    shpool: ShieldedPool,
    pool: PoolType,
    receiver_value: u64,
    change: u64,
    test_mempool: bool,
) where
    CC: ConductChain,
{
    timestamped_test_log(format!("starting a {shpool:?} to {pool} test").as_str());

    let mut environment = CC::setup().await;

    // Put distance between the chain tip and the height-5 NU6.1/6.2
    // co-activation before building transactions. Zebra rejects
    // orchard-OUTPUT transactions built adjacent to the boundary
    // ("could not validate orchard proof ... until the next chain tip
    // block") while accepting sapling-output equivalents; the
    // tip_spend_rejection discrimination suite pins this differential
    // and exonerates note age, anchors, and note selection.
    for _ in 0..5 {
        environment.increase_chain_height().await;
    }

    let mut primary = environment.create_faucet().await;
    let mut secondary = environment.create_client().await;
    let mut tertiary = environment.create_client().await;

    let expected_fee = fee_tables::one_to_one(Some(shpool), pool, true);

    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut primary,
        vec![(
            &get_base_address(&secondary, PoolType::Shielded(shpool)).await,
            receiver_value + change + expected_fee,
            None,
        )],
        vec![&mut secondary],
        test_mempool,
    )
    .await
    .unwrap();

    let (recorded_fee, recorded_value, recorded_change) =
        with_assertions::assure_propose_send_bump_sync_all_recipients(
            &mut environment,
            &mut secondary,
            vec![(
                &get_base_address(&tertiary, pool).await,
                receiver_value,
                None,
            )],
            vec![&mut tertiary],
            test_mempool,
        )
        .await
        .unwrap();
    assert_eq!(
        (recorded_fee, recorded_value, recorded_change),
        (
            Zatoshis::from_u64(expected_fee).unwrap(),
            Zatoshis::from_u64(receiver_value).unwrap(),
            Zatoshis::from_u64(change).unwrap()
        )
    );
}
