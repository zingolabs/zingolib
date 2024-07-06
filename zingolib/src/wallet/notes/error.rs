//! Errors arising from the Output interface, probably to be extended with other note-related
//! functionality soon.
use zcash_primitives::transaction::TxId;

#[allow(missing_docs)] // Error is self documenting
#[derive(Debug, thiserror::Error)]
pub enum OutputConstructionError {
    #[error("")]
    ZeroInputTransaction(TxId),
}
