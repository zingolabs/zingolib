use orchard::tree::MerkleHashOrchard;
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
    let mut transparent_actions = 0;
    let mut sapling_actions = 0;
    let mut orchard_actions = 0;
    match source_protocol {
        Sapling => sapling_actions += 1,
        Orchard => orchard_actions += 1,
    }
    match target_pool {
        Transparent => transparent_actions += 1,
        Shielded(Sapling) => sapling_actions += 1,
        Shielded(Orchard) => orchard_actions += 1,
    }
    let whattype = MARGINAL_FEE
        * std::cmp::max(
            transparent_actions + sapling_actions + orchard_actions,
            GRACE_ACTIONS,
        );
    whattype
        .expect("actions expected to be in numberical range")
        .into_u64()
}
