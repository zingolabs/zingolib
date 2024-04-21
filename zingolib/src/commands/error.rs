use std::fmt;

#[derive(Debug)]
pub(crate) enum CommandError {
    ArgsNotJson(json::Error),
    SingleArgNotJsonArray(String),
    ParseIntFromString(std::num::ParseIntError),
    UnexpectedType(String),
    MissingKey(String),
    InvalidArguments,
    IncompatibleMemo,
    InvalidMemo(String),
    NonJsonNumberForAmount(String),
    ConversionFailed(crate::utils::ConversionError),
    InvalidPool,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CommandError::*;

        match self {
            ArgsNotJson(e) => write!(f, "failed to parse argument. {}", e),
            ParseIntFromString(e) => write!(f, "failed to parse argument. {}", e),
            UnexpectedType(e) => write!(f, "arguments cannot be parsed to expected type. {}", e),
            MissingKey(key) => write!(f, "json array is missing \"{}\" key.", key),
            InvalidArguments => write!(f, "arguments given are invalid."),
            IncompatibleMemo => {
                write!(f, "memo's cannot be sent to transparent addresses.")
            }
            InvalidMemo(e) => write!(f, "failed to interpret memo. {}", e),
            NonJsonNumberForAmount(e) => write!(f, "invalid argument. expected a number. {}", e),
            SingleArgNotJsonArray(e) => {
                write!(f, "argument cannot be parsed to a json array. {}", e)
            }
            ConversionFailed(e) => write!(f, "conversion failed. {}", e),
            InvalidPool => write!(f, "invalid pool."),
        }
    }
}
