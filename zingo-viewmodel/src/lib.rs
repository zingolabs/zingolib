#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! Editorial reductions over zingolib's canonical wallet data.
//!
//! zingolib answers "what happened on-chain to this wallet"; this crate
//! answers "what should a consumer show for it". A reduction lives in
//! zingolib only when computing it needs privilege (wallet secrets,
//! protocol constants, or chain state) AND no two reasonable consumers
//! could want a different answer; everything else — value-transfer
//! construction, self-send value zeroing, memo display policy,
//! per-recipient rollups, status wording — is editorial and lives here.
//!
//! Consumers (zingo-mobile, zingo-cli) opt in through extension traits
//! that preserve the method names, signatures, and JSON shapes they
//! already use; zingolib itself never depends on this crate. See
//! `docs/adr/0013-editorial-reductions-in-zingo-viewmodel.md`.

pub mod ext;
pub mod finsight;
#[cfg(feature = "testutils")]
pub mod testutils;
pub mod value_transfer;

pub use ext::{LightClientViewModelExt, LightWalletViewModelExt};
pub use value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransfer, ValueTransferKind, ValueTransfers,
};
