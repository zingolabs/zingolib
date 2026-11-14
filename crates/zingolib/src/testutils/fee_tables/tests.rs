use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::testutils::fee_tables;

#[test]
fn the_conventional_action_count_floors_at_the_grace_actions() {
    for logical_actions in 0..=zip317::GRACE_ACTIONS {
        assert_eq!(
            fee_tables::conventional_actions(logical_actions),
            zip317::GRACE_ACTIONS
        );
    }
    assert_eq!(
        fee_tables::conventional_actions(zip317::GRACE_ACTIONS + 1),
        zip317::GRACE_ACTIONS + 1
    );
}

#[test]
fn the_conventional_fee_floors_at_the_minimum_fee() {
    for logical_actions in 0..=zip317::GRACE_ACTIONS {
        assert_eq!(
            fee_tables::conventional_fee(logical_actions),
            zip317::MINIMUM_FEE.into_u64()
        );
    }
}

#[test]
fn the_conventional_fee_grows_by_one_marginal_fee_per_action_past_the_grace() {
    for logical_actions in zip317::GRACE_ACTIONS + 1..=zip317::GRACE_ACTIONS + 4 {
        assert_eq!(
            fee_tables::conventional_fee(logical_actions)
                - fee_tables::conventional_fee(logical_actions - 1),
            zip317::MARGINAL_FEE.into_u64()
        );
    }
}

#[test]
fn a_bundle_with_any_flow_pads_to_two_actions_and_an_empty_bundle_to_none() {
    assert_eq!(fee_tables::padded_actions(0, 0), 0);
    assert_eq!(fee_tables::padded_actions(1, 0), 2);
    assert_eq!(fee_tables::padded_actions(0, 1), 2);
    assert_eq!(fee_tables::padded_actions(2, 1), 2);
    assert_eq!(fee_tables::padded_actions(3, 1), 3);
    assert_eq!(fee_tables::padded_actions(1, 4), 4);
}

#[test]
fn a_shield_pays_one_action_per_transparent_coin_plus_the_padded_shielded_output() {
    for transparent_inputs in 1..=4 {
        assert_eq!(
            fee_tables::shield(transparent_inputs),
            fee_tables::conventional_fee(transparent_inputs + fee_tables::padded_actions(0, 1))
        );
    }
}

#[test]
fn an_orchard_receiver_and_an_ironwood_receiver_are_one_cell_for_every_source() {
    for source in [
        None,
        Some(ShieldedPool::Sapling),
        Some(ShieldedPool::Orchard),
        Some(ShieldedPool::Ironwood),
    ] {
        for change in [false, true] {
            assert_eq!(
                fee_tables::one_to_one(source, PoolType::ORCHARD, change),
                fee_tables::one_to_one(source, PoolType::IRONWOOD, change),
                "source {source:?}, change {change}"
            );
        }
    }
}

#[test]
fn a_deshield_pays_one_action_per_transparent_output_plus_the_padded_shielded_spend() {
    for transparent_outputs in 1..=4 {
        assert_eq!(
            fee_tables::deshield(transparent_outputs),
            fee_tables::conventional_fee(transparent_outputs + fee_tables::padded_actions(1, 1))
        );
    }
}

#[test]
fn the_paid_actions_invert_the_conventional_fee() {
    for logical_actions in 0..=zip317::GRACE_ACTIONS + 4 {
        let fee = Zatoshis::from_u64(fee_tables::conventional_fee(logical_actions))
            .expect("a conventional fee stays within the money range");
        assert_eq!(
            fee_tables::paid_actions(fee),
            fee_tables::conventional_actions(logical_actions)
        );
    }
}

#[test]
fn an_ironwood_note_spent_to_an_orchard_receiver_with_change_pays_the_minimum_fee() {
    assert_eq!(
        fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::ORCHARD, true),
        zip317::MINIMUM_FEE.into_u64()
    );
}

#[test]
fn an_orchard_source_keeps_its_change_in_the_orchard_bundle() {
    let orchard_spend_with_change = fee_tables::padded_actions(1, 1);
    assert_eq!(
        fee_tables::one_to_one(Some(ShieldedPool::Orchard), PoolType::TRANSPARENT, true),
        fee_tables::conventional_fee(orchard_spend_with_change + 1)
    );
    assert_eq!(
        fee_tables::one_to_one(Some(ShieldedPool::Orchard), PoolType::SAPLING, true),
        fee_tables::conventional_fee(orchard_spend_with_change + fee_tables::padded_actions(0, 1))
    );
    assert_eq!(
        fee_tables::one_to_one(Some(ShieldedPool::Orchard), PoolType::ORCHARD, true),
        fee_tables::conventional_fee(orchard_spend_with_change + fee_tables::padded_actions(0, 1))
    );
}
