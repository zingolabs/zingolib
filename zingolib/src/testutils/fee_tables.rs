//! zip317 specifications

use std::cmp::max;

use zcash_primitives::transaction::fees::zip317::{GRACE_ACTIONS, MARGINAL_FEE};
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::wallet::migration::split;

const PADDED_BUNDLE_ACTIONS: usize = split::MIN_BUNDLE_ACTIONS as usize;

/// Estimates the fee the proposer charges for a one-input send on an
/// NU6.3-active chain (the default test era, ADR 0009), per ZIP 317
/// (<https://zips.z.cash/zip-0317>) as the pinned zcash_client_backend
/// implements it for V6 transactions. Adjudicated against the proposer by
/// the `pool_matrix` tests in `lightclient/propose.rs`:
///
/// - A shielded payment to an Orchard receiver lands in the Ironwood
///   bundle (the Ironwood receiver of a UA *is* its Orchard receiver), so
///   `PoolType::ORCHARD` and `PoolType::IRONWOOD` targets are the same
///   cell.
/// - An Orchard-note spend stays in the Orchard bundle, so an Orchard
///   source paying an Orchard receiver builds two bundles, while an
///   Ironwood source and its payment share one.
/// - Change follows upstream's `select_change_pool` and stays in the
///   source's own bundle: Orchard change for an Orchard source, Ironwood
///   change when any Ironwood flow exists, else Sapling change.
/// - Each shielded bundle with any flow pads to its own two-action floor, while
///   the transparent contribution is unpadded.
#[must_use]
pub fn one_to_one(
    source_protocol: Option<ShieldedPool>,
    target_pool: PoolType,
    mut change: bool,
) -> u64 {
    if source_protocol.is_none() && target_pool == PoolType::TRANSPARENT {
        change = false;
    }

    let transparent_inputs: usize = 0;
    let mut transparent_outputs: usize = 0;
    let mut sapling_inputs: usize = 0;
    let mut sapling_outputs: usize = 0;
    let mut orchard_inputs: usize = 0;
    let mut orchard_outputs: usize = 0;
    let mut ironwood_inputs: usize = 0;
    let mut ironwood_outputs: usize = 0;
    match source_protocol {
        Some(ShieldedPool::Sapling) => sapling_inputs += 1,
        Some(ShieldedPool::Orchard) => orchard_inputs += 1,
        Some(ShieldedPool::Ironwood) => ironwood_inputs += 1,
        None => {}
    }
    match target_pool {
        PoolType::Transparent => transparent_outputs += 1,
        PoolType::Shielded(ShieldedPool::Sapling) => sapling_outputs += 1,
        PoolType::Shielded(ShieldedPool::Orchard | ShieldedPool::Ironwood) => {
            ironwood_outputs += 1;
        }
    }
    if change {
        if orchard_inputs > 0 {
            orchard_outputs += 1;
        } else if ironwood_inputs + ironwood_outputs == 0 {
            // sapling change
            sapling_outputs += 1;
        } else {
            // ironwood change
            ironwood_outputs += 1;
        }
    }

    let contribution_transparent = max(transparent_outputs, transparent_inputs);
    let contribution_sapling = padded_actions(sapling_inputs, sapling_outputs);
    let contribution_orchard = padded_actions(orchard_inputs, orchard_outputs);
    let contribution_ironwood = padded_actions(ironwood_inputs, ironwood_outputs);
    conventional_fee(
        contribution_transparent
            + contribution_sapling
            + contribution_orchard
            + contribution_ironwood,
    )
}

#[must_use]
pub fn conventional_actions(logical_actions: usize) -> usize {
    max(logical_actions, GRACE_ACTIONS)
}

#[must_use]
pub fn conventional_fee(logical_actions: usize) -> u64 {
    (MARGINAL_FEE * conventional_actions(logical_actions))
        .expect("a transaction's action count keeps its fee within the money range")
        .into_u64()
}

#[must_use]
pub fn padded_actions(inputs: usize, outputs: usize) -> usize {
    let actions = max(inputs, outputs);
    if actions > 0 {
        max(actions, PADDED_BUNDLE_ACTIONS)
    } else {
        0
    }
}

#[must_use]
pub fn shield(transparent_inputs: usize) -> u64 {
    conventional_fee(transparent_inputs + padded_actions(0, 1))
}

#[must_use]
pub fn deshield(transparent_outputs: usize) -> u64 {
    conventional_fee(transparent_outputs + padded_actions(1, 1))
}

#[must_use]
pub fn paid_actions(fee: Zatoshis) -> usize {
    usize::try_from(fee.into_u64() / MARGINAL_FEE.into_u64())
        .expect("a fee within the money range pays fewer actions than usize holds")
}

#[cfg(test)]
mod tests {
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
                fee_tables::conventional_fee(
                    transparent_outputs + fee_tables::padded_actions(1, 1)
                )
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
            fee_tables::conventional_fee(
                orchard_spend_with_change + fee_tables::padded_actions(0, 1)
            )
        );
        assert_eq!(
            fee_tables::one_to_one(Some(ShieldedPool::Orchard), PoolType::ORCHARD, true),
            fee_tables::conventional_fee(
                orchard_spend_with_change + fee_tables::padded_actions(0, 1)
            )
        );
    }
}
