use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum ZingoLibError {
    NoWalletLocation,
    MetadataUnderflow,
    InternalWriteBufferError(std::io::Error),
    WriteFileError(std::io::Error),
    EmptySaveBuffer,
}

impl std::fmt::Display for ZingoLibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ZingoLibError::*;
        match self {
            NoWalletLocation => write!(
                f,
                "No wallet location! (compiled for native rust, wallet location expected)"
            ),
            MetadataUnderflow => write!(
                f,
                "Metadata underflow! Recorded metadata shows greater output than input value. This may be because input notes are prebirthday."
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
        }
    }
}

impl From<ZingoLibError> for String {
    fn from(value: ZingoLibError) -> Self {
        format!("{value}")
    }
}

impl Error for ZingoLibError {}
