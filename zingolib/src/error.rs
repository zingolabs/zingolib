#[derive(Debug, PartialEq, Eq, derive_more::From)]
pub enum ZingoLibError {
    NoWalletLocation,
    ZatMathError(ZatMathError),
}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

#[derive(Debug, PartialEq, Eq)]
pub enum ZatMathError {
    Overflow,
    Underflow,
}
