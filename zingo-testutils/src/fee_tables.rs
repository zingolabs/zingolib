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

/// wip
pub fn one_to_one_no_change(source_protocol: ShieldedProtocol, target_pool: PoolType) -> u64 {
    let mut transparent_inputs = 0;
    let mut transparent_outputs = 0;
    let mut sapling_inputs = 0;
    let mut sapling_outputs = 0;
    let mut orchard_inputs = 0;
    let mut orchard_outputs = 0;
    match source_protocol {
        Sapling => sapling_inputs += 1,
        Orchard => orchard_inputs += 1,
    }
    match target_pool {
        Transparent => transparent_outputs += 1,
        Shielded(Sapling) => sapling_outputs += 1,
        Shielded(Orchard) => orchard_outputs += 1,
    }
    let mut contribution_transparent = max(transparent_outputs, transparent_inputs);
    let mut contribution_sapling = max(sapling_outputs, sapling_inputs);
    let mut contribution_orchard = max(orchard_outputs, orchard_inputs);
    let whattype = MARGINAL_FEE
        * max(
            contribution_transparent + contribution_sapling + contribution_orchard,
            GRACE_ACTIONS,
        );
    whattype
        .expect("actions expected to be in numberical range")
        .into_u64()
}
