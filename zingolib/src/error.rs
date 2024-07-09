//! TODO: Add Mod Description Here!

use std::error::Error;
use std::fmt;

use zcash_primitives::transaction::TxId;

/// Holds zingolib error types
#[derive(Debug)]
pub enum ZingoLibError {
    /// TODO: Add Doc Comment Here!
    UnknownError,
    /// TODO: Add Doc Comment Here!
    Error(String),
    /// TODO: Add Doc Comment Here!
    NoWalletLocation,
    /// TODO: Add Doc Comment Here!
    MetadataUnderflow(String),
    /// TODO: Add Doc Comment Here!
    InternalWriteBufferError(std::io::Error),
    /// TODO: Add Doc Comment Here!
    WriteFileError(std::io::Error),
    /// TODO: Add Doc Comment Here!
    EmptySaveBuffer,
    /// TODO: Add Doc Comment Here!
    CantReadWallet(std::io::Error),
    /// TODO: Add Doc Comment Here!
    NoSuchTxId(TxId),
    /// TODO: Add Doc Comment Here!
    NoSuchSaplingOutputInTx(TxId, u32),
    /// TODO: Add Doc Comment Here!
    NoSuchOrchardOutputInTx(TxId, u32),
    /// TODO: Add Doc Comment Here!
    NoSuchNullifierInTx(TxId),
    /// TODO: Add Doc Comment Here!
    MissingOutputIndex(TxId),
    /// TODO: Add Doc Comment Here!
    CouldNotDecodeMemo(std::io::Error),
}

/// TODO: Add Doc Comment Here!
pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

impl ZingoLibError {
    /// TODO: Add Doc Comment Here!
    pub fn handle<T>(self) -> ZingoLibResult<T> {
        log::error!("{}", self);
        Err(self)
    }
}

impl std::fmt::Display for ZingoLibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ZingoLibError::*;
        match self {
            UnknownError => write!(
                f,
                "UnknownError",
            ),
            Error(string) => write!(
                f,
                "Error: {}",
                string,
            ),
            NoWalletLocation => write!(
                f,
                "No wallet location! (compiled for native rust, wallet location expected)"
            ),
            MetadataUnderflow(explanation) => write!(
                f,
                "Metadata underflow! Recorded metadata shows greater output than input value. This may be because input notes are prebirthday. {}",
                explanation,
            ),
            InternalWriteBufferError(err) => write!(
                f,
                "Internal save error! {} ",
                err,
            ),
            WriteFileError(err) => write!(
                f,
                "Could not write to wallet save file. Was this erroneously attempted in mobile?, instead of native save buffer handling? Is there a permission issue? {} ",
                err,
            ),
            EmptySaveBuffer => write!(
                f,
                "Empty save buffer. probably save_external was called before save_internal_rust. this is handled by save_external."
            ),
            CantReadWallet(err) => write!(
                f,
                "Cant read wallet. Corrupt file. Or maybe a backwards version issue? {}",
                err,
            ),
            NoSuchTxId(txid) => write!(
                f,
                "Cant find TxId {}!",
                txid,
            ),
            NoSuchSaplingOutputInTx(txid, output_index) => write!(
                f,
                "Cant find note with sapling output_index {} in TxId {}",
                output_index,
                txid,
            ),
            NoSuchOrchardOutputInTx(txid, output_index) => write!(
                f,
                "Cant find note with orchard output_index {} in TxId {}",
                output_index,
                txid,
            ),
            NoSuchNullifierInTx(txid) => write!(
                f,
                "Cant find that Nullifier in TxId {}",
                txid,
            ),
            CouldNotDecodeMemo(err) => write!(
                f,
                "Could not decode memo. Zingo plans to support foreign memo formats soon. {}",
                err,
            ),
            MissingOutputIndex(txid) => write!(
                f,
                "{txid} is missing output_index for note, cannot mark change"
            ),
        }
    }
}

impl From<ZingoLibError> for String {
    fn from(value: ZingoLibError) -> Self {
        format!("{value}")
    }
}

impl Error for ZingoLibError {}

/// Errors associated with builder patterns in production code
#[derive(thiserror::Error, Debug)]
pub enum BuildError {
    /// build failed. missing field.
    #[error("build failed. missing field {0}")]
    MissingField(String),
}
