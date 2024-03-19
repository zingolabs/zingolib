use std::error::Error;
use std::fmt;

use zcash_primitives::transaction::TxId;

#[derive(Debug)]
pub enum ZingoLibError {
    UnknownError,
    Error(String),
    NoWalletLocation,
    MetadataUnderflow(String),
    InternalWriteBufferError(std::io::Error),
    WriteFileError(std::io::Error),
    EmptySaveBuffer,
    CantReadWallet(std::io::Error),
    NoSuchTxId(TxId),
    NoSuchSaplingOutputInTx(TxId, u32),
    NoSuchOrchardOutputInTx(TxId, u32),
    NoSuchNullifierInTx(TxId),
    MissingOutputIndex(TxId),
    CouldNotDecodeMemo(std::io::Error),
    ViewkeyCantSpend,
}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

impl ZingoLibError {
    pub fn handle<T>(self) -> ZingoLibResult<T> {
        log::error!("{}", self);
        Err(self)
    }
}

impl std::fmt::Display for ZingoLibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ZingoLibError::*;
        write!(
            f,
            "{}",
            match self {
                UnknownError => format!(
                    "UnknownError"
                ),
                Error(string) => format!(
                    "Error: {}",
                    string,
                ),
                NoWalletLocation => format!(
                    "No wallet location! (compiled for native rust, wallet location expected)",
                ),
                MetadataUnderflow(explanation) => format!(
                    "Metadata underflow! Recorded metadata shows greater output than input value. This may be because input notes are prebirthday. {}",
                    explanation,
                ),
                InternalWriteBufferError(err) => format!(
                    "Internal save error! {} ",
                    err,
                ),
                WriteFileError(err) => format!(
                    "Could not write to wallet save file. Was this erroneously attempted in mobile?, instead of native save buffer handling? Is there a permission issue? {} ",
                    err,
                ),
                EmptySaveBuffer => format!(
                    "Empty save buffer. probably save_external was called before save_internal_rust. this is handled by save_external."
                ),
                CantReadWallet(err) => format!(
                    "Cant read wallet. Corrupt file. Or maybe a backwards version issue? {}",
                    err,
                ),
                NoSuchTxId(txid) => format!(
                    "Cant find TxId {}!",
                    txid,
                ),
                NoSuchSaplingOutputInTx(txid, output_index) => format!(
                    "Cant find note with sapling output_index {} in TxId {}",
                    output_index,
                    txid,
                ),
                NoSuchOrchardOutputInTx(txid, output_index) => format!(
                    "Cant find note with orchard output_index {} in TxId {}",
                    output_index,
                    txid,
                ),
                NoSuchNullifierInTx(txid) => format!(
                    "Cant find that Nullifier in TxId {}",
                    txid,
                ),
                CouldNotDecodeMemo(err) => format!(
                    "Could not decode memo. Zingo plans to support foreign memo formats soon. {}",
                    err,
                ),
                MissingOutputIndex(txid) => format!(
                    "{txid} is missing output_index for note, cannot mark change"
                ),
                ViewkeyCantSpend => format!(
                    "viewkey cannot spend",
                ),
            }
        )
    }
}

impl From<ZingoLibError> for String {
    fn from(value: ZingoLibError) -> Self {
        format!("{value}")
    }
}

impl Error for ZingoLibError {}
