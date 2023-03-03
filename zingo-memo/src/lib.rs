use zcash_client_backend::address::UnifiedAddress;
use zcash_primitives::consensus::BlockHeight;

pub mod memo_serde;
pub mod utils;

/// A parsed memo. Currently there is only one version of this protocol,
/// which is a list of UAs. The main use-case for this is to record the
/// UAs sent from, as the blockchain only records the pool-specific receiver
/// corresponding to the key we sent with.
#[non_exhaustive]
#[derive(Debug)]
pub enum ParsedMemo {
    Version0 {
        uas: Vec<UnifiedAddress>,
    },
    Version1 {
        uas: Vec<UnifiedAddress>,
        transaction_heights_and_indexes: Vec<(BlockHeight, usize)>,
    },
}

#[cfg(test)]
mod test_vectors;
