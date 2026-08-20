//! The consumer funnel over `zingo-netutils` (ADR 0024, decision 7).
//!
//! Consumers declare exactly one wallet dependency — zingolib, pinned by
//! git rev — and reach every netutils item they legitimately need through
//! these re-exports instead of holding a second direct dependency. One
//! dependent means one feature configuration and one compiled copy, which
//! is the fix for zingo-mobile's capability-less duplicate netutils copy
//! (issue #2566).
//!
//! The funnel's root is zingolib itself — the shallow option of issue
//! #2578's deferred choice. The deeper alternative (pepper-sync as the
//! sole direct dependent, re-exporting netutils verbatim) was declined at
//! implementation time: it would graft the transmit surface onto the sync
//! engine's public API for no consumer-visible gain, since out-of-repo
//! consumers see only zingolib either way.
//!
//! The set is curated, not the whole crate: what consumers legitimately
//! need is the indexer client and its trait, the process-level crypto
//! provider hook, the curated indexer lists, and the temporal calibration.
//! Widening the set is a deliberate API decision, not a convenience.
#![forbid(unsafe_code)]

// GetClientError joins the set deliberately: zingo-cli's `network on`
// consent act connects mid-session via `set_indexer_uri` and models the
// failure as a typed variant rather than a rendered string.
pub use zingo_netutils::{GetClientError, GrpcIndexer, Indexer, ensure_default_crypto_provider};

// Status joins on the same terms, and for the same reason: zingo-cli's
// clearnet server probe names the stage that failed, and an RPC rejection is
// the indexer's own verdict, so the probe carries that verdict rather than a
// rendering of it. A consumer holding a typed rejection cannot reach it
// without this, because the funnel is a consumer's only path to the crate.
pub use zingo_netutils::Status;

pub use zingo_netutils::indexers;
pub use zingo_netutils::time;
