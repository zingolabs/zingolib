use std::error::Error;
use std::fmt;

use zcash_primitives::transaction::TxId;

#[derive(Debug)]
pub enum ZingoLibError {
    UnknownError,
    NoWalletLocation,
    MetadataUnderflow(String),
    InternalWriteBufferError(std::io::Error),
    WriteFileError(std::io::Error),
    EmptySaveBuffer,
    CantReadWallet(std::io::Error),
    NoSuchTxId(TxId),
    NoSuchSaplingOutputInTxId(TxId, u32),
    NoSuchOrchardOutputInTxId(TxId, u32),
    CouldNotDecodeMemo(std::io::Error),
}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

impl ZingoLibError {
    pub fn handle<T>(self) -> ZingoLibResult<T> {
        log::error!("{}", self);

        #[cfg(feature = "test-features")]
        {
            panic!("{}", self);
        }

        #[cfg(not(feature = "test-features"))]
        {
            Err(self)
        }
    }
}

impl std::fmt::Display for ZingoLibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ZingoLibError::*;
        match self {
            UnknownError => write!(
                f,
                "UnknownError"
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
            NoSuchSaplingOutputInTxId(txid, output_index) => write!(
                f,
                "Cant find note with sapling output_index {} in TxId {}",
                output_index,
                txid,
            ),
            NoSuchOrchardOutputInTxId(txid, output_index) => write!(
                f,
                "Cant find note with orchard output_index {} in TxId {}",
                output_index,
                txid,
            ),
            CouldNotDecodeMemo(err) => write!(
                f,
                "Could not decode memo. Zingo plans to support foreign memo formats soon. {}",
                err,
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
