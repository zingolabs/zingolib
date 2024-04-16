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

impl NoteRecordIdentifier {
    pub fn from_parts(txid: TxId, pool: PoolType, index: u32) -> Self {
        NoteRecordIdentifier { txid, pool, index }
    }
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

#[cfg(any(feature = "test", feature = "test-features"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_client_backend::PoolType;
    use zcash_primitives::transaction::TxId;

    use crate::test_framework::mocks::build_method;

    use super::NoteRecordIdentifier;

    pub(crate) struct NoteRecordIdentifierBuilder {
        txid: Option<TxId>,
        pool: Option<PoolType>,
        index: Option<u32>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl NoteRecordIdentifierBuilder {
        pub fn new() -> Self {
            Self {
                txid: None,
                pool: None,
                index: None,
            }
        }
        // Methods to set each field
        build_method!(txid, TxId);
        build_method!(pool, PoolType);
        build_method!(index, u32);

        pub fn randomize_txid(self) -> Self {
            self.txid(crate::test_framework::mocks::mock_txid())
        }

        // Build method
        pub fn build(self) -> NoteRecordIdentifier {
            NoteRecordIdentifier::from_parts(
                self.txid.unwrap(),
                self.pool.unwrap(),
                self.index.unwrap(),
            )
        }
    }

    impl Default for NoteRecordIdentifierBuilder {
        fn default() -> Self {
            Self::new()
                .txid(TxId::from_bytes([0u8; 32]))
                .pool(PoolType::Shielded(
                    zcash_client_backend::ShieldedProtocol::Orchard,
                ))
                .index(0)
        }
    }
}
