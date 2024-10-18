#![warn(missing_docs)]
//! Zingo sync engine prototype
//!
//! Entrypoint: [`crate::sync::sync`]

pub mod client;
pub(crate) mod keys;
#[allow(missing_docs)]
pub mod primitives;
pub(crate) mod scan;
pub mod sync;
pub mod traits;
pub mod witness;
