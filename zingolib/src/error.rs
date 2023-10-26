use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use crate::wallet::{data::PoolNullifier, Pool};

#[derive(Debug, PartialEq, Eq, derive_more::From, derive_more::Display)]
pub enum ZingoLibError {
    NoWalletLocation,
    /// An overflow or underflow occurred during amount arithmatic
    ZatMathError(ZatMathError),
    /// Tried to do an operation without the appropriate wallet capability
    InsufficientCapability(InsufficientCapability),
    NoShieldedReciever,
    TransactionCreationError(TransactionCreationError),
    /// An internal wallet error indicates something went wrong
    /// inside zingolib, as opposed to other errors that could be caused by
    /// bad input. If you see an internal wallet error, something has gone wrong
    InternalWalletError(InternalWalletError),
}

#[derive(Debug, PartialEq, Eq, derive_more::Display)]
pub enum InternalWalletError {
    /// Address derevation failed
    DerevationError(secp256k1::Error),
    #[display(
        fmt = "Missing {} nullifier",
        "match _0 { \
            PoolNullifier::Sapling(_) => \"sapling\", \
            PoolNullifier::Orchard(_) => \"orchard\", \
        }"
    )]
    MissingNullifier(PoolNullifier),
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
