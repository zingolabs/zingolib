use zcash_primitives::transaction::TxId;

#[allow(missing_docs)] // Error is self documenting
#[derive(Debug, thiserror::Error)]
pub enum OutputConstructionError {
    #[error("")]
    ZeroInputTransaction(TxId),
}
