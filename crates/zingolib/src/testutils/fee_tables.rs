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
mod tests;
