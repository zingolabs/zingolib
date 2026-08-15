//! zip317 specifications

use std::cmp::max;

use zcash_primitives::transaction::fees::zip317::{GRACE_ACTIONS, MARGINAL_FEE};
use zcash_protocol::{PoolType, ShieldedPool};

/// Estimates the fee the proposer charges for a one-input send on an
/// NU6.3-active chain (the default test era, ADR 0009), per ZIP 317
/// (<https://zips.z.cash/zip-0317>) as the in-tree spend pipeline
/// implements it for V6 transactions. Adjudicated against real proposals
/// (probe run, 2026-07-13):
///
/// - A shielded payment to an Orchard receiver lands in the Ironwood
///   bundle (the Ironwood receiver of a UA *is* its Orchard receiver), so
///   `PoolType::ORCHARD` and `PoolType::IRONWOOD` targets are the same
///   cell.
/// - Ironwood-note spends are counted in the *orchard* bundle view: the
///   wallet labels V3 notes `PoolType::ORCHARD` for input selection (the
///   builder detects Ironwood inputs by note version, see
///   `wallet/zcb_traits.rs`), so Orchard and Ironwood sources price
///   identically. The upstream fee view therefore charges an
///   orchard-bundle contribution for the inputs *and* an ironwood-bundle
///   contribution for the outputs. If upstream later folds these into one
///   bundle's actions, this table (and the proposer) will drop in lockstep
///   on the dependency bump.
/// - Change follows upstream's `select_change_pool`: Orchard when any
///   orchard-view flow exists (which includes Ironwood sources), else
///   Sapling when a sapling flow exists. Orchard-pool change is then
///   routed into the ironwood bundle.
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

    // Flows per bundle view. Orchard and Ironwood sources both contribute
    // inputs to the orchard view; shielded payments to Orchard receivers
    // land in the ironwood view.
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
        PoolType::Shielded(ShieldedPool::Orchard) => orchard_outputs += 1,
        PoolType::Shielded(ShieldedPool::Ironwood) => ironwood_outputs += 1,
    }
    if change {
        if orchard_inputs > 0 {
            // orchard migrated into ironwood
            ironwood_outputs += 1;
        } else if ironwood_inputs + ironwood_outputs == 0 {
            // sapling change
            sapling_outputs += 1;
        } else {
            // ironwood change
            ironwood_outputs += 1;
        }
    }

    let pad = |inputs: usize, outputs: usize| {
        let actions = max(inputs, outputs);
        if actions > 0 { max(actions, 2) } else { 0 }
    };
    let contribution_transparent = max(transparent_outputs, transparent_inputs);
    let contribution_sapling = pad(sapling_inputs, sapling_outputs);
    let mut contribution_orchard = pad(orchard_inputs, orchard_outputs);
    if source_protocol == Some(ShieldedPool::Orchard) && target_pool == PoolType::ORCHARD {
        contribution_orchard *= 2;
    }
    let contribution_ironwood = pad(ironwood_inputs, ironwood_outputs);
    let total_fee = MARGINAL_FEE
        * max(
            contribution_transparent
                + contribution_sapling
                + contribution_orchard
                + contribution_ironwood,
            GRACE_ACTIONS,
        );
    total_fee
        .expect("actions expected to be in numerical range")
        .into_u64()
}
