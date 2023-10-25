use crate::wallet::Pool;

#[derive(Debug, PartialEq, Eq, derive_more::From, derive_more::Display)]
pub enum ZingoLibError {
    NoWalletLocation,
    ZatMathError(ZatMathError),
    InsufficientCapability(InsufficientCapability),
    DerevationError(secp256k1::Error),
    NoShieldedReciever,
}

impl std::error::Error for ZingoLibError {}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub enum ZatMathError {
    Overflow,
    Underflow,
}

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub struct InsufficientCapability {
    pool: Pool,
    required_capability: CapabilityKind,
    held_capability: CapabilityKind,
}

impl InsufficientCapability {
    pub fn need_viewkey(pool: Pool) -> Self {
        Self {
            pool,
            required_capability: CapabilityKind::ViewCapable,
            held_capability: CapabilityKind::NoCapability,
        }
    }

    pub fn need_spendkey(pool: Pool, held_capability: CapabilityKind) -> Self {
        Self {
            pool,
            required_capability: CapabilityKind::SpendCapable,
            held_capability,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    NoCapability,
    ViewCapable,
    SpendCapable,
}
