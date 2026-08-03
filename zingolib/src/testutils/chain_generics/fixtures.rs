//! these functions are each meant to be 'test-in-a-box'
//! simply plug in a mock server as a chain conductor and provide some values

use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::chain_generics::with_assertions;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::timestamped_test_log;

/// sends back and forth several times, including sends to transparent
pub async fn send_shield_cycle<CC>(n: u64)
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let primary_fund = 1_000_000;
    let mut primary = environment.fund_client_orchard(primary_fund).await;

    let mut secondary = environment.create_client().await;
    let secondary_taddr = get_base_address(&secondary, PoolType::Transparent).await;

    for _ in 0..n {
        let (recorded_fee, recorded_value, recorded_change) =
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut environment,
                &mut primary,
                vec![
                    (&secondary_taddr, 100_000, None),
                    (&secondary_taddr, 4_000, None),
                ],
                vec![&mut secondary],
                false,
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

        let (recorded_fee, recorded_value) = with_assertions::assure_propose_shield_bump_sync(
            &mut environment,
            &mut secondary,
            false,
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

        let (recorded_fee, recorded_value, recorded_change) =
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut environment,
                &mut secondary,
                vec![(
                    &get_base_address(&primary, PoolType::Shielded(ShieldedPool::Orchard)).await,
                    50_000,
                    None,
                )],
                vec![&mut primary],
                false,
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
