use std::fmt;

#[derive(Debug)]
pub(crate) enum CommandError {
    ArgsNotJson(json::Error),
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
    ConversionFailed(crate::utils::ConversionError),
    InvalidPool,
    MultipleReceivers,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CommandError::*;

        match self {
            ArgsNotJson(e) => write!(f, "failed to parse argument. {}", e),
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
            InvalidPool => write!(f, "invalid pool."),
            MultipleReceivers => write!(f, "'send all' can only accept one receiver."),
        }
    }
}
