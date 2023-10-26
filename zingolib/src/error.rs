use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use crate::wallet::Pool;

#[derive(Debug, PartialEq, Eq, derive_more::From, derive_more::Display)]
pub enum ZingoLibError {
    NoWalletLocation,
    ZatMathError(ZatMathError),
    InsufficientCapability(InsufficientCapability),
    DerevationError(secp256k1::Error),
    NoShieldedReciever,
    TransactionCreationError(TransactionCreationError),
}

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub enum TransactionCreationError {
    #[display(
        fmt = "insufficient balance: need {}, have {}",
        "u64::from(*needed)",
        "u64::from(*held)"
    )]
    InsufficientBalance {
        needed: NonNegativeAmount,
        held: NonNegativeAmount,
    },
}

impl std::error::Error for ZingoLibError {}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub enum ZatMathError {
    Overflow,
    Underflow,
}

pub type ZatMathResult<T> = Result<T, ZatMathError>;

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
#[display(fmt = "Need {pool} {required_capability}, have {pool} {held_capability}")]
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

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub enum CapabilityKind {
    NoCapability,
    ViewCapable,
    SpendCapable,
}
