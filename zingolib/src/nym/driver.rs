//! The Mixnet Mode session driver's types (ADR 0024, decision 2): the
//! typed status snapshot the one shared subscription delivers, the
//! per-session start policy, and the provisioning strategy a session hands
//! the driver at its go-online moment.
//!
//! The driver itself is [`crate::lightclient::LightClient::start_mixnet_session`];
//! this module holds its vocabulary. Status reaches subscribers through one
//! session-level `tokio::sync::watch` channel: the supervisor's tasks
//! publish transport transitions directly, the wallet's slot methods publish
//! the slot states, and nothing polls. The channel's keep-only-latest
//! semantics are the publication-sequencing guard — a stale snapshot can
//! never overtake a newer one.
#![forbid(unsafe_code)]

use std::sync::Arc;

use crate::nym::{DeathReport, MixnetMode};

/// One observation of Mixnet Mode for subscribers: the mode plus the
/// evidence scoped to it. Every field beyond the mode is `None` outside
/// the one mode it belongs to, exactly as the pull accessors are gated.
///
/// In-process-plain by ratified decision (2026-07-28): the wire
/// serialization of this snapshot — and its golden pins — is minted in the
/// boundary-carrier phase, when out-of-process consumers exist to pin
/// against. The fields are chosen so serde bolts on without reshaping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixnetStatus {
    /// The Mixnet Mode at this observation.
    pub mode: MixnetMode,
    /// The local SOCKS5 address, present exactly while ready.
    pub socks5_addr: Option<String>,
    /// The transport's latest bootstrap progress line, present only while
    /// bootstrapping, so a subscriber can narrate the connect race.
    pub bootstrap_detail: Option<String>,
    /// The latched death read whole, present only while died.
    pub death: Option<DeathReport>,
}

impl MixnetStatus {
    /// The snapshot of a slot state that carries no transport — unattached
    /// or switched off — where every transport-scoped field is absent.
    pub(crate) fn slot_only(mode: MixnetMode) -> Self {
        MixnetStatus {
            mode,
            socks5_addr: None,
            bootstrap_detail: None,
            death: None,
        }
    }
}

/// The shared sending half of the session's one status channel. The wallet
/// owns it for the session's life and clones it into each transport's
/// supervisor tasks, so successive transports (an enable after a disable)
/// publish into the same channel a subscriber holds.
pub(crate) type StatusPublisher = Arc<tokio::sync::watch::Sender<MixnetStatus>>;

/// A fresh session-level status channel, opened in the ground state:
/// unattached, which carries no online intent. Receivers come from
/// `subscribe()` on the sender, so only the publisher is kept.
pub(crate) fn status_publisher() -> StatusPublisher {
    let (sender, _initial_receiver) =
        tokio::sync::watch::channel(MixnetStatus::slot_only(MixnetMode::Unattached));
    Arc::new(sender)
}

/// How a session's go-online moment treats the mixnet (ADR 0024, consent
/// at start). A two-variant enum rather than a boolean so the parameter
/// says what the opt-out is: a per-session consent act, never a persisted
/// preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixnetStartPolicy {
    /// The default for every connected session: force Mixnet Mode on by
    /// provisioning the transport now, so the bootstrap overlaps sync and
    /// the mixnet-only surfaces are protected from the session's start.
    ForcedOn,
    /// The user's startup opt-out — the explicit act that reaches
    /// switched off, recording the same per-session clearnet consent as an
    /// in-session toggle-off. Never persisted.
    OptedOutThisSession,
}

/// How the transport comes to exist at the go-online moment — the one
/// dimension along which consumers legitimately differ (ADR 0011, ADR
/// 0024): a desktop session spawns the bundled binary, a platform that
/// hosts the proxy itself hands the wallet an address to attach to.
#[derive(Debug)]
pub enum ProvisionStrategy<'a> {
    /// Spawn the bundled `nym-proxy` binary, resolving its path from the
    /// consumer's platform hints through the one precedence rule in
    /// [`crate::nym::provision`].
    Spawn(crate::nym::provision::SpawnHints<'a>),
    /// Attach to an already-running, platform-hosted SOCKS5 endpoint.
    Attach {
        /// The endpoint's socket address.
        socks5_addr: &'a str,
    },
}
