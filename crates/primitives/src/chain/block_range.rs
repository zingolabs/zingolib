use super::BlockId;

// This type is built by the caller, not received from a server, so there is nothing in it to
// validate: a wire-format conversion is always valid by construction. It exists so pepper-sync's
// own code never names a wire type, not to guard against malformed input the way a type parsed
// from the wire does.

/// A range of blocks to request from a server, with ascending order for `start <= end` and
/// descending order for `start > end`, optionally scoped to specific shielded pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRange {
    /// The first block of the range.
    pub start: BlockId,
    /// The last block of the range.
    pub end: BlockId,
    /// The pools to include compact data for. Empty means every pool.
    pub pool_types: Vec<zcash_protocol::PoolType>,
}

/// The wire format's `PoolType` enum encoding for `pool`.
pub fn pool_type_wire_value(pool: zcash_protocol::PoolType) -> i32 {
    use zcash_protocol::{PoolType, ShieldedPool};
    match pool {
        PoolType::Transparent => 1,
        PoolType::Shielded(ShieldedPool::Sapling) => 2,
        PoolType::Shielded(ShieldedPool::Orchard) => 3,
        PoolType::Shielded(ShieldedPool::Ironwood) => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::pool_type_wire_value;
    use zcash_protocol::{PoolType, ShieldedPool};

    #[test]
    fn every_pool_maps_to_its_own_wire_value() {
        assert_eq!(pool_type_wire_value(PoolType::Transparent), 1);
        assert_eq!(
            pool_type_wire_value(PoolType::Shielded(ShieldedPool::Sapling)),
            2
        );
        assert_eq!(
            pool_type_wire_value(PoolType::Shielded(ShieldedPool::Orchard)),
            3
        );
        assert_eq!(
            pool_type_wire_value(PoolType::Shielded(ShieldedPool::Ironwood)),
            4
        );
    }
}
