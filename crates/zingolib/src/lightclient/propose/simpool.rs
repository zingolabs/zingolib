use zcash_protocol::{PoolType, ShieldedPool};

use crate::lightclient::LightClient;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};

/// A wallet holding one `source`-pool note `underflow_amount` short of
/// a 100_000 send to `pool` reports the exact shortfall.
async fn insufficient(source: ShieldedPool, underflow_amount: u64, pool: PoolType) {
    let expected_fee = fee_tables::one_to_one(Some(source), pool, true);
    let secondary_fund = 100_000 + expected_fee - underflow_amount;
    let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
    let wallet = match source {
        ShieldedPool::Ironwood => builder.ironwood_note(secondary_fund),
        ShieldedPool::Orchard => builder.orchard_note(secondary_fund),
        ShieldedPool::Sapling => builder.sapling_note(secondary_fund),
    }
    .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let tertiary_fund = 100_000;
    assert_eq!(
        from_inputs::propose(
            &mut client,
            vec![(
                synthetic_wallet::external_address(pool).as_str(),
                tertiary_fund,
                None
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

/// A wallet with no funds at all reports the full amount-plus-fee need.
async fn unfunded_to(try_amount: u64, pool: PoolType) {
    let expected_fee = fee_tables::one_to_one(None, pool, true);
    let wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        from_inputs::propose(
            &mut client,
            vec![(
                synthetic_wallet::external_address(pool).as_str(),
                try_amount,
                None
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

#[tokio::test]
async fn insufficient_1_ironwood_to_ironwood() {
    insufficient(ShieldedPool::Ironwood, 1, PoolType::IRONWOOD).await;
}
#[tokio::test]
async fn insufficient_1_ironwood_to_sapling() {
    insufficient(ShieldedPool::Ironwood, 1, PoolType::SAPLING).await;
}
#[tokio::test]
async fn insufficient_1_ironwood_to_transparent() {
    insufficient(ShieldedPool::Ironwood, 1, PoolType::Transparent).await;
}
#[tokio::test]
async fn insufficient_10_000_ironwood_to_ironwood() {
    insufficient(ShieldedPool::Ironwood, 10_000, PoolType::IRONWOOD).await;
}
#[tokio::test]
async fn insufficient_10_000_ironwood_to_sapling() {
    insufficient(ShieldedPool::Ironwood, 10_000, PoolType::SAPLING).await;
}
#[tokio::test]
async fn insufficient_10_000_ironwood_to_transparent() {
    insufficient(ShieldedPool::Ironwood, 10_000, PoolType::Transparent).await;
}
#[tokio::test]
async fn no_fund_1_000_000_to_ironwood() {
    unfunded_to(1_000_000, PoolType::IRONWOOD).await;
}
#[tokio::test]
async fn no_fund_1_000_000_to_sapling() {
    unfunded_to(1_000_000, PoolType::SAPLING).await;
}
#[tokio::test]
async fn no_fund_1_000_000_to_transparent() {
    unfunded_to(1_000_000, PoolType::Transparent).await;
}
#[tokio::test]
async fn insufficient_1_sapling_to_ironwood() {
    insufficient(ShieldedPool::Sapling, 1, PoolType::IRONWOOD).await;
}
#[tokio::test]
async fn insufficient_1_sapling_to_sapling() {
    insufficient(ShieldedPool::Sapling, 1, PoolType::SAPLING).await;
}
#[tokio::test]
async fn insufficient_1_sapling_to_transparent() {
    insufficient(ShieldedPool::Sapling, 1, PoolType::Transparent).await;
}
#[tokio::test]
async fn insufficient_10_000_sapling_to_ironwood() {
    insufficient(ShieldedPool::Sapling, 10_000, PoolType::IRONWOOD).await;
}
#[tokio::test]
async fn insufficient_10_000_sapling_to_sapling() {
    insufficient(ShieldedPool::Sapling, 10_000, PoolType::SAPLING).await;
}
#[tokio::test]
async fn insufficient_10_000_sapling_to_transparent() {
    insufficient(ShieldedPool::Sapling, 10_000, PoolType::Transparent).await;
}
