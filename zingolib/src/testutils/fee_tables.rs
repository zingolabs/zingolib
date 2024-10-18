//! zip317 specifications

use std::cmp::max;

use zcash_client_backend::PoolType;
use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;
use zcash_primitives::transaction::fees::zip317::GRACE_ACTIONS;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

/// estimates a fee based on the zip317 protocol rules
/// <https://zips.z.cash/zip-0317>
pub fn one_to_one(
    source_protocol: Option<ShieldedProtocol>,
    target_pool: PoolType,
    change: bool,
) -> u64 {
    let transparent_inputs = 0;
    let mut transparent_outputs = 0;
    let mut sapling_inputs = 0;
    let mut sapling_outputs = 0;
    let mut orchard_inputs = 0;
    let mut orchard_outputs = 0;
    match source_protocol {
        Some(Sapling) => sapling_inputs += 1,
        Some(Orchard) => orchard_inputs += 1,
        _ => {}
    }
    match target_pool {
        Transparent => transparent_outputs += 1,
        Shielded(Sapling) => sapling_outputs += 1,
        Shielded(Orchard) => orchard_outputs += 1,
    }
    if change {
        if orchard_inputs + orchard_outputs == 0 {
            // sapling change
            sapling_outputs += 1;
        } else {
            //orchard change
            orchard_outputs += 1;
        }
    }
    if sapling_outputs > 0 || sapling_inputs > 0 {
        sapling_outputs = max(sapling_outputs, 2); //MIN_SHIELDED_OUTPUTS;
    }
    let mut orchard_actions = max(orchard_inputs, orchard_outputs);
    if orchard_actions > 0 {
        orchard_actions = max(orchard_actions, 2); //MIN_SHIELDED_OUTPUTS;
    }
    let contribution_transparent = max(transparent_outputs, transparent_inputs);
    let contribution_sapling = max(sapling_outputs, sapling_inputs);
    let contribution_orchard = orchard_actions;
    let total_fee = MARGINAL_FEE
        * max(
            contribution_transparent + contribution_sapling + contribution_orchard,
            GRACE_ACTIONS,
        );
    total_fee
        .expect("actions expected to be in numberical range")
        .into_u64()
}
