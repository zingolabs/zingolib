//! The consumer funnel over `pepper-sync` (ADR 0024, decision 7).
//!
//! Consumers declare exactly one wallet dependency and reach every
//! pepper-sync item they legitimately need through these re-exports
//! instead of holding a second direct pin — the same doctrine as
//! [`crate::netutils`], applied to the sync engine. The 2026-08-03 seed
//! audit found the gap this module closes: every governed consumer
//! imported pepper-sync directly, and two of the items below are the
//! declared types of this crate's own API —
//! [`crate::lightclient::LightClient::sync_mode`] returns [`SyncMode`],
//! and the lightclient error type wraps [`SyncModeError`] — so a
//! consumer without them cannot name a funneled method's return type or
//! destructure a funneled error.
//!
//! The set is curated, not the whole crate: the sync-mode vocabulary and
//! its error, the key-id trait consumers use to read scopes, the
//! transparent key module, the per-pool note types the summaries expose,
//! the free `sync_status` reader, and the confirmation-status vocabulary
//! every summary carries. Widening the set is a deliberate API decision,
//! not a convenience.
#![forbid(unsafe_code)]

pub use pepper_sync::error::SyncModeError;
pub use pepper_sync::keys::transparent;
pub use pepper_sync::wallet::{IronwoodNote, KeyIdInterface, OrchardNote, SaplingNote, SyncMode};

pub use zingo_status::confirmation_status::ConfirmationStatus;
