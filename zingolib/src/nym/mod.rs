//! Nym mixnet IP-obfuscation transport for the Transmission and price-fetch
//! surfaces — seam B of `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This module is the pure, CI-testable core: the [`MixnetMode`] tri-state,
//! the witness-rotation [`broadcast`](broadcast::broadcast) over an injected
//! [`Transmitter`](broadcast::Transmitter) and random-number generator, and
//! the curated Broadcast Indexer list. Nothing here depends on the nym
//! crates, so the rotation and failover logic runs without a reachable
//! mixnet; the live nym-sdk SOCKS5 transport is a later increment that
//! implements `Transmitter`.
#![forbid(unsafe_code)]

pub mod broadcast;
pub mod broadcast_indexers;
mod mode;

pub use mode::MixnetMode;
