pub mod interface;
pub use interface::NoteInterface;
pub use interface::ShieldedNoteInterface;
pub mod transparent;
pub use transparent::TransparentNote;
pub mod sapling;
pub use sapling::SaplingNote;
pub mod orchard;
pub use orchard::OrchardNote;

use zcash_client_backend::PoolType;
use zcash_primitives::transaction::TxId;

/// This triple of values uniquely identifies an entry on a zcash blockchain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteRecordIdentifier {
    pub txid: TxId,
    pub pool: PoolType,
    pub index: u32,
}

impl std::fmt::Display for NoteRecordIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "txid {}, {:?}, index {}",
            self.txid, self.pool, self.index,
        )
    }
}
