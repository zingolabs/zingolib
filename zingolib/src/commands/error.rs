use std::fmt;

#[derive(Debug)]
pub(crate) enum CommandError {
    ArgsNotJson(json::Error),
    #[cfg(feature = "zip317")]
    #[allow(dead_code)]
    ArgNotJsonOrValidAddress,
    SingleArgNotJsonArray(String),
    EmptyJsonArray,
    ParseIntFromString(std::num::ParseIntError),
    UnexpectedType(String),
    MissingKey(String),
    InvalidArguments,
    IncompatibleMemo,
    InvalidMemo(String),
    NonJsonNumberForAmount(String),
    ConversionFailed(crate::utils::error::ConversionError),
    #[cfg(not(feature = "zip317"))]
    InvalidPool,
    #[cfg(feature = "zip317")]
    #[allow(dead_code)]
    MultipleReceivers,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CommandError::*;

        match self {
            ArgsNotJson(e) => write!(f, "failed to parse argument. {}", e),
            #[cfg(feature = "zip317")]
            ArgNotJsonOrValidAddress => write!(
                f,
                "argument cannot be converted to a valid address or parsed as json."
            ),
            SingleArgNotJsonArray(e) => {
                write!(f, "argument cannot be parsed to a json array. {}", e)
            }
            EmptyJsonArray => write!(f, "json array has no arguments"),
            ParseIntFromString(e) => write!(f, "failed to parse argument. {}", e),
            UnexpectedType(e) => write!(f, "arguments cannot be parsed to expected type. {}", e),
            MissingKey(key) => write!(f, "json array is missing \"{}\" key.", key),
            InvalidArguments => write!(f, "arguments given are invalid."),
            IncompatibleMemo => {
                write!(f, "memo's cannot be sent to transparent addresses.")
            }
            InvalidMemo(e) => write!(f, "failed to interpret memo. {}", e),
            NonJsonNumberForAmount(e) => write!(f, "invalid argument. expected a number. {}", e),
            ConversionFailed(e) => write!(f, "conversion failed. {}", e),
            #[cfg(not(feature = "zip317"))]
            InvalidPool => write!(f, "invalid pool."),
            #[cfg(feature = "zip317")]
            MultipleReceivers => write!(f, "'send all' can only accept one receiver."),
        }
    }
}

impl std::error::Error for CommandError {}
