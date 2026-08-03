#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! Editorial reductions over zingolib's canonical wallet data, and the
//! funnel consumers declare as their single wallet dependency.
//!
//! zingolib answers "what happened on-chain to this wallet"; this crate
//! answers "what should a consumer show for it". A reduction lives in
//! zingolib only when computing it needs privilege (wallet secrets,
//! protocol constants, or chain state) AND no two reasonable consumers
//! could want a different answer; everything else — value-transfer
//! construction, self-send value zeroing, memo display policy,
//! per-recipient rollups — is editorial and lives here.
//!
//! Consumers opt in through extension traits that preserve the method
//! names, signatures, and JSON shapes they already use; zingolib itself
//! never depends on this crate.
//!
//! Per ADR 0024 rule 7 (amendment 2026-08-03) this crate is also the
//! funnel: the re-exports below mirror zingolib's consumer surface
//! path-for-path, so a governed consumer declares exactly one wallet
//! dependency and repoints by renaming it.

pub mod ext;
pub mod finsight;
#[cfg(feature = "testutils")]
pub mod testutils;
pub mod value_transfer;

pub use ext::{LightClientViewModelExt, LightWalletViewModelExt};
pub use value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransfer, ValueTransferKind, ValueTransfers,
};

pub use zingolib::{
    ActivationHeights, DEVELOPER_DONATION_ADDRESS, SaplingParams, ZENNIES_FOR_ZINGO_AMOUNT,
    ZENNIES_FOR_ZINGO_DONATION_ADDRESS, ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
    ZENNIES_FOR_ZINGO_TESTNET_ADDRESS, config, connectivity, data, ensure_default_crypto_provider,
    get_zennies_for_zingo_address, git_description, indexers, lightclient, netutils, utils, wallet,
};

#[cfg(feature = "nym")]
pub use zingolib::nym;

#[cfg(feature = "testutils")]
pub use zingolib::mocks;
