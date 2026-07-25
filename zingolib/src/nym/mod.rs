//! Nym mixnet IP-obfuscation transport for the Transmission and price-fetch
//! surfaces, seam B of `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This module holds the mixnet control and policy logic: the [`MixnetMode`]
//! tri-state, the fail-closed [`route`] resolver shared by every mixnet-only
//! surface, the escalating fan-out [`broadcast`] over an injected per-arm
//! runner and random-number generator, the curated Broadcast Indexer list, and
//! the [`supervisor`] that owns the spawned `nym-proxy` child. The fan-out
//! orchestrates the shared per-submission resilience policy across rounds, and
//! because its arm runner and RNG are injected, the round, escalation, and cap
//! logic runs in CI without a reachable mixnet or real time.
#![forbid(unsafe_code)]

pub mod broadcast;
pub mod broadcast_indexers;
mod mode;
pub mod probe;
pub mod route;
pub mod supervisor;

pub use mode::{IP_CORRELATION_DISCLAIMER, MixnetMode};
pub use route::{MixnetNotReady, MixnetRoute, resolve_route};
pub use supervisor::{MixnetProxy, MixnetProxyError};
