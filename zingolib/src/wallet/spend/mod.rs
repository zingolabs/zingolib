//! The in-tree spend pipeline (ADR 0010).
//!
//! This module replaces zingolib's use of `zcash_client_backend`'s
//! proposal and transaction-construction machinery with orchestration the
//! wallet owns. It is layered functionally, effects only at the edges: a
//! pure plan layer maps a read-only wallet view and a payment request to
//! a [`proposal::Proposal`]; a wallet-pure build layer maps a proposal,
//! keys, provers, and pre-extracted witnesses to signed transactions; and
//! a single apply site performs the wallet mutations. Fee arithmetic,
//! transaction building, and proving stay upstream in `zcash_primitives`
//! and `zcash_proofs` — this module decides only which notes to spend,
//! what change to make, and which outputs each step gets.
//!
//! P2 of the plan delivers the owned types below; the plan, build, and
//! apply layers land in the following phases.

pub mod apply;
pub mod build;
pub(crate) mod fee;
pub mod op_return;
pub mod plan;
pub mod proposal;

pub use op_return::{MAX_OP_RETURN_DATA_BYTES, OpReturnData, OpReturnDataError};
pub use proposal::{
    ChangeValue, Proposal, ProposalShapeError, ShieldProposal, ShieldedInput, Step,
    TexTransferProposal, TransferProposal, TransparentInput,
};
