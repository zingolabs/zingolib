use zcash_client_backend::{PoolType, ShieldedProtocol};

/// calculates the expected fee for a transaction with no change
/// wip
pub fn one_to_one_no_change(source_pool: ShieldedProtocol, target_pool: PoolType) -> u64 {
    10_000
}
