use zcash_protocol::{PoolType, ShieldedPool};

use crate::testutils::lightclient::from_inputs::transaction_request_from_send_inputs;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};
use crate::wallet::keys::unified::ReceiverSelection;

/// Paying a unified address must target its best receiver: Orchard
/// whenever the UA carries an orchard receiver, Sapling only when that
/// is the best on offer. This is the guarantee the LocalNet test
/// `diversified_addresses_receive_funds_in_best_pool` enforced with a
/// full zebrad+zainod network. The proposal's payment-pool map states
/// it directly from synthetic wallet data alone.
#[test]
fn proposal_targets_best_pool_per_unified_address() {
    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(100_000)
        .build();
    let chain = wallet.chain_type;

    let (_, orchard_only) = wallet
        .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
        .unwrap();
    let (_, all_shielded) = wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    let (_, sapling_only) = wallet
        .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
        .unwrap();
    let orchard_only = orchard_only.encode(&chain);
    let all_shielded = all_shielded.encode(&chain);
    let sapling_only = sapling_only.encode(&chain);

    let request = transaction_request_from_send_inputs(vec![
        (orchard_only.as_str(), 10_000, None),
        (all_shielded.as_str(), 10_000, None),
        (sapling_only.as_str(), 10_000, None),
    ])
    .expect("valid send inputs form a request");

    let proposal = wallet
        .create_send_proposal(request, zip32::AccountId::ZERO)
        .expect("synthetic wallet data supports proposing");

    let step = proposal.steps().first();
    let pools = step.payment_pools();
    assert_eq!(
        pools[&0],
        PoolType::Shielded(ShieldedPool::Ironwood),
        "orchard-only UA must be paid in ironwood"
    );
    assert_eq!(
        pools[&1],
        PoolType::Shielded(ShieldedPool::Ironwood),
        "all-shielded UA must be paid in its best pool, ironwood"
    );
    assert_eq!(
        pools[&2],
        PoolType::Shielded(ShieldedPool::Sapling),
        "sapling-only UA must be paid in sapling"
    );
}

/// Migrated from libtonode `propose_orchard_dust_to_sapling`: a wallet
/// holding an ordinary orchard note and a dust note can propose a
/// cross-pool send to a sapling address.
/// FIXME: does not assert dust was included in the proposal (carried
/// over from the original).
#[test]
fn propose_orchard_dust_to_sapling() {
    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(100_000)
        .orchard_note(4_000)
        .build();

    let sapling_destination = synthetic_wallet::external_address(PoolType::SAPLING);

    let request =
        transaction_request_from_send_inputs(vec![(sapling_destination.as_str(), 10_000, None)])
            .expect("valid send inputs form a request");

    wallet
        .create_send_proposal(request, zip32::AccountId::ZERO)
        .expect("orchard funds propose cleanly to a sapling destination");
}

/// Proposing a spend of existing funds works from wallet data alone, with
/// no network. Formerly `#[ignore]`d ("for some reason this does not
/// work without network"): it loaded an example wallet fixture, and
/// fixtures deserialize without the confirmed-transaction state
/// proposing requires. The synthetic builder fabricates that state.
#[test]
fn propose_100_000_to_self() {
    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(200_000)
        .build();

    let pool = PoolType::Shielded(ShieldedPool::Orchard);
    let self_address = wallet.get_address(pool);

    let receivers = vec![(self_address.as_str(), 100_000, None)];
    let request = transaction_request_from_send_inputs(receivers)
        .expect("actually all of this logic oughta be internal to propose");

    wallet
        .create_send_proposal(request, zip32::AccountId::ZERO)
        .expect("can propose from existing data");
}
