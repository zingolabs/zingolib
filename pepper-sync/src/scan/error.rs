use zcash_primitives::{block::BlockHash, consensus::BlockHeight};

/// Scan errors.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Continuity error.
    #[error("Continuity error. {0}")]
    ContinuityError(#[from] ContinuityError),
}

/// Block continuity errors.
#[derive(Debug, thiserror::Error)]
pub enum ContinuityError {
    /// Height discontinuity.
    #[error("Height discontinuity. Block with height {height} is not continuous with previous block height {previous_block_height}")]
    HeightDiscontinuity {
        height: BlockHeight,
        previous_block_height: BlockHeight,
    },
    /// Hash discontinuity.
    #[error("Hash discontinuity. Block prev_hash {prev_hash} with height {height} does not match previous block hash {previous_block_hash}")]
    HashDiscontinuity {
        height: BlockHeight,
        prev_hash: BlockHash,
        previous_block_hash: BlockHash,
    },
}
