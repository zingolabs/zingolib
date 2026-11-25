#![forbid(unsafe_code)]

/// Re-exported from `zl-primitives`: the chain-reply domain vocabulary lives in its own crate,
/// free of the wallet stack, so `zl-netutils` can name it without depending on `zl-common`.
/// `zl-common`'s own consumers (`pepper-sync`, `zingolib`) keep using this same path.
pub use zl_primitives::chain;
pub mod config;
#[doc(hidden)]
pub mod engine;
pub mod keys;
pub mod memo;
pub mod price;
pub mod serialization;
pub mod shardtree_ext;
pub mod status;
pub mod sync;
pub mod wallet;
mod witness;
