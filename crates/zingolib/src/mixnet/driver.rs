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

use crate::mixnet::{DeathReport, Indicator, acquire, provision, supervisor};

/// One observation of Mixnet Mode for subscribers: the mode plus the
/// evidence scoped to it. Every field beyond the mode is `None` outside
/// the one mode it belongs to, exactly as the pull accessors are gated.
///
/// This is the wire snapshot the neon boundary carries to zingo-pc, the
/// boundary-carrier consumer ADR 0024 sequences last. The serde form omits
/// every absent field, so the `mode` token alone discriminates and each state
/// carries only its own evidence. The wire is pinned by the `wire_contract`
/// golden test below, the same way [`Indicator`]'s five tokens are pinned in
/// `mode.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RawMixnetStatus")]
pub struct MixnetStatus {
    /// The Mixnet Mode at this observation.
    pub mode: Indicator,
    /// The local SOCKS5 address, present exactly while ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks5_addr: Option<std::net::SocketAddr>,
    /// The Exit Node identities the ready transport bound, present only
    /// while ready.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub exits: Vec<crate::mixnet::ExitNodeId>,
    /// The transport's latest bootstrap progress line, present only while
    /// bootstrapping, so a subscriber can narrate the connect race.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_detail: Option<String>,
    /// The latched death read whole, present only while died.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub death: Option<DeathReport>,
}

/// The lifted form before mode-scoping, through whose mirror serde reads
/// the typed fields — whose own deserializers refuse a malformed address
/// or exit entry as suspicious, because producer and consumer are pinned
/// to one code revision — so that [`TryFrom`] can refuse any evidence
/// outside its one mode.
#[derive(serde::Deserialize)]
struct RawMixnetStatus {
    mode: Indicator,
    #[serde(default)]
    socks5_addr: Option<std::net::SocketAddr>,
    #[serde(default)]
    exits: Vec<crate::mixnet::ExitNodeId>,
    #[serde(default)]
    bootstrap_detail: Option<String>,
    #[serde(default)]
    death: Option<DeathReport>,
}

impl TryFrom<RawMixnetStatus> for MixnetStatus {
    type Error = String;

    fn try_from(raw: RawMixnetStatus) -> Result<Self, Self::Error> {
        fn stray(field: &str, mode: Indicator) -> String {
            format!("{field} is not evidence for mode {}", mode.as_str())
        }
        // Route evidence rides every routable mode: earned Ready and
        // stale-proven PreviouslyProvenThisEpoch, which routes the same.
        if raw.socks5_addr.is_some() && !raw.mode.is_ready() {
            return Err(stray("socks5_addr", raw.mode));
        }
        if !raw.exits.is_empty() && !raw.mode.is_ready() {
            return Err(stray("exits", raw.mode));
        }
        if raw.bootstrap_detail.is_some() && raw.mode != Indicator::Bootstrapping {
            return Err(stray("bootstrap_detail", raw.mode));
        }
        if raw.death.is_some() && raw.mode != Indicator::Died {
            return Err(stray("death", raw.mode));
        }
        Ok(MixnetStatus {
            mode: raw.mode,
            socks5_addr: raw.socks5_addr,
            exits: raw.exits,
            bootstrap_detail: raw.bootstrap_detail,
            death: raw.death,
        })
    }
}

impl MixnetStatus {
    /// The snapshot of a slot state that carries no transport — unattached
    /// or switched off — where every transport-scoped field is absent.
    pub(crate) fn slot_only(mode: Indicator) -> Self {
        MixnetStatus {
            mode,
            socks5_addr: None,
            exits: Vec::new(),
            bootstrap_detail: None,
            death: None,
        }
    }

    /// A status carrying only the evidence its mode offers — route fields
    /// ride the routable modes and the death rides `Died` alone — so every
    /// published shape satisfies the wire guard by construction.
    pub(crate) fn evidenced(
        mode: Indicator,
        socks5_addr: Option<std::net::SocketAddr>,
        exits: Vec<crate::mixnet::ExitNodeId>,
        death: Option<DeathReport>,
    ) -> Self {
        let routable = mode.is_ready();
        MixnetStatus {
            mode,
            socks5_addr: if routable { socks5_addr } else { None },
            exits: if routable { exits } else { Vec::new() },
            bootstrap_detail: None,
            death: if mode == Indicator::Died { death } else { None },
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
        tokio::sync::watch::channel(MixnetStatus::slot_only(Indicator::Unattached));
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
    /// switched off and sets the session's transmit policy to nakednet, so
    /// sends travel over the indexer while the mixnet-only surfaces refuse.
    /// Never persisted.
    OptedOutThisSession,
}

/// How the transport comes to exist at the go-online moment — the one
/// dimension along which consumers legitimately differ (ADR 0011, ADR
/// 0024): a desktop session spawns the bundled binary, a mobile platform
/// that hosts the proxy itself hands the wallet an address to attach to.
#[derive(Debug)]
pub enum ProvisionStrategy<'a> {
    /// Spawn the bundled `nym-proxy` binary, resolving its path from the
    /// consumer's platform hints through the one precedence rule in
    /// [`crate::mixnet::provision`].
    Spawn(crate::mixnet::provision::SpawnHints<'a>),
    /// Attach to an already-running, mobile-platform-hosted SOCKS5 endpoint.
    Attach {
        /// The endpoint's socket address.
        socks5_addr: &'a str,
        /// The Exit Node identities the mobile platform host reports as bound.
        exits: &'a [crate::mixnet::ExitNodeId],
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStarted {
    OptedOut,
    Spawned {
        resolution: provision::ProxyResolution,
    },
    Attached {
        socks5_addr: String,
    },
}

impl std::fmt::Display for SessionStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStarted::OptedOut => {
                f.write_str("the startup opt-out, which provisions nothing")
            }
            SessionStarted::Spawned { resolution } => {
                write!(f, "a nym-proxy spawned at {resolution}")
            }
            SessionStarted::Attached { socks5_addr } => {
                write!(f, "an attach to the host endpoint '{socks5_addr}'")
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStartError {
    #[error("the nym proxy at {resolution} did not start")]
    Spawn {
        resolution: provision::ProxyResolution,
        #[source]
        source: acquire::TransportError,
    },
    #[error("the attach to the host endpoint '{socks5_addr}' failed")]
    Attach {
        socks5_addr: String,
        #[source]
        source: supervisor::MixnetProxyError,
    },
}

/// The golden wire pins for the status snapshot (ADR 0024, the boundary-carrier
/// mint). Each state is pinned literally in both directions: serializing the
/// canonical value produces exactly the bytes, and those bytes lift back to the
/// value. A drift here is a breaking change to zingo-pc's neon boundary, the
/// same contract `mode.rs`'s `wire_contract` keeps for the five mode tokens.
#[cfg(test)]
mod wire_contract;
