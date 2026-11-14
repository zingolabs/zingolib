use zcash_protocol::{PoolType, ShieldedPool};

use crate::lightclient::LightClient;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};

/// One matrix cell: a wallet holding a single `source` note of exactly
/// `receiver_value + change + fee` proposes a `receiver_value` send to
/// a `pool` destination, and the proposal's fee and change land on the
/// fee table's prediction to the zatoshi.
async fn matrix_case(source: ShieldedPool, pool: PoolType, receiver_value: u64, change: u64) {
    let expected_fee = fee_tables::one_to_one(Some(source), pool, change > 0);
    let funding = receiver_value + change + expected_fee;
    let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
    let wallet = match source {
        ShieldedPool::Ironwood => builder.ironwood_note(funding),
        ShieldedPool::Orchard => builder.orchard_note(funding),
        ShieldedPool::Sapling => builder.sapling_note(funding),
    }
    .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination = synthetic_wallet::external_address(pool);
    let proposal = from_inputs::propose(
        &mut client,
        vec![(destination.as_str(), receiver_value, None)],
    )
    .await
    .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    assert_eq!(u64::from(step.balance().fee_required()), expected_fee);
    let proposed_change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(proposed_change, change);
    let payment: u64 = step
        .transaction_request()
        .payments()
        .values()
        .map(|payment| u64::from(payment.amount().expect("matrix payments carry amounts")))
        .sum();
    assert_eq!(payment, receiver_value);
}

macro_rules! pool_matrix_case {
    ($name:ident, $source:expr, $receiver:expr, $send_value:expr, $change:expr) => {
        #[tokio::test]
        async fn $name() {
            matrix_case($source, $receiver, $send_value, $change).await;
        }
    };
}

pool_matrix_case!(
    sapling_sends_to_transparent,
    ShieldedPool::Sapling,
    PoolType::TRANSPARENT,
    10_000,
    1_000
);
pool_matrix_case!(
    sapling_sends_to_sapling,
    ShieldedPool::Sapling,
    PoolType::SAPLING,
    10_000,
    1_000
);
pool_matrix_case!(
    sapling_sends_to_ironwood,
    ShieldedPool::Sapling,
    PoolType::IRONWOOD,
    10_000,
    1_000
);
pool_matrix_case!(
    ironwood_sends_to_transparent,
    ShieldedPool::Ironwood,
    PoolType::TRANSPARENT,
    10_000,
    1_000
);
pool_matrix_case!(
    ironwood_sends_to_sapling,
    ShieldedPool::Ironwood,
    PoolType::SAPLING,
    10_000,
    1_000
);
pool_matrix_case!(
    ironwood_sends_to_ironwood,
    ShieldedPool::Ironwood,
    PoolType::IRONWOOD,
    10_000,
    1_000
);
pool_matrix_case!(
    sapling_sends_to_transparent_no_change,
    ShieldedPool::Sapling,
    PoolType::TRANSPARENT,
    10_000,
    0
);
pool_matrix_case!(
    sapling_sends_to_sapling_no_change,
    ShieldedPool::Sapling,
    PoolType::SAPLING,
    10_000,
    0
);
pool_matrix_case!(
    sapling_sends_to_ironwood_no_change,
    ShieldedPool::Sapling,
    PoolType::IRONWOOD,
    10_000,
    0
);
pool_matrix_case!(
    orchard_sends_to_transparent_no_change,
    ShieldedPool::Orchard,
    PoolType::TRANSPARENT,
    10_000,
    0
);
pool_matrix_case!(
    orchard_sends_to_sapling_no_change,
    ShieldedPool::Orchard,
    PoolType::SAPLING,
    10_000,
    0
);
pool_matrix_case!(
    orchard_sends_to_orchard_no_change,
    ShieldedPool::Orchard,
    PoolType::ORCHARD,
    10_000,
    0
);
pool_matrix_case!(
    orchard_sends_to_ironwood_no_change,
    ShieldedPool::Orchard,
    PoolType::IRONWOOD,
    10_000,
    0
);
pool_matrix_case!(
    ironwood_sends_to_transparent_no_change,
    ShieldedPool::Ironwood,
    PoolType::TRANSPARENT,
    10_000,
    0
);
pool_matrix_case!(
    ironwood_sends_to_sapling_no_change,
    ShieldedPool::Ironwood,
    PoolType::SAPLING,
    10_000,
    0
);
pool_matrix_case!(
    ironwood_sends_to_ironwood_no_change,
    ShieldedPool::Ironwood,
    PoolType::IRONWOOD,
    10_000,
    0
);
pool_matrix_case!(
    sapling_sends_to_orchard,
    ShieldedPool::Sapling,
    PoolType::ORCHARD,
    10_000,
    1_000
);
pool_matrix_case!(
    sapling_sends_to_orchard_no_change,
    ShieldedPool::Sapling,
    PoolType::ORCHARD,
    10_000,
    0
);
pool_matrix_case!(
    ironwood_sends_to_orchard,
    ShieldedPool::Ironwood,
    PoolType::ORCHARD,
    10_000,
    1_000
);
pool_matrix_case!(
    ironwood_sends_to_orchard_no_change,
    ShieldedPool::Ironwood,
    PoolType::ORCHARD,
    10_000,
    0
);
pool_matrix_case!(
    orchard_sends_to_transparent,
    ShieldedPool::Orchard,
    PoolType::TRANSPARENT,
    10_000,
    1_000
);
pool_matrix_case!(
    orchard_sends_to_sapling,
    ShieldedPool::Orchard,
    PoolType::SAPLING,
    10_000,
    1_000
);
pool_matrix_case!(
    orchard_sends_to_orchard,
    ShieldedPool::Orchard,
    PoolType::ORCHARD,
    10_000,
    1_000
);
pool_matrix_case!(
    orchard_sends_to_ironwood,
    ShieldedPool::Orchard,
    PoolType::IRONWOOD,
    10_000,
    1_000
);
// 546 zatoshis is the canonical transparent dust threshold. The
// proposer permits it; whether a relay accepts it is the network's
// business, and zebra's mempool dust rule was the reason this row's
// LocalNet ancestor sent 546 rather than 1.
pool_matrix_case!(
    sapling_sends_to_transparent_minimum_value,
    ShieldedPool::Sapling,
    PoolType::TRANSPARENT,
    546,
    0
);
pool_matrix_case!(
    sapling_sends_to_transparent_boundary_values,
    ShieldedPool::Sapling,
    PoolType::TRANSPARENT,
    49_999,
    9_999
);
