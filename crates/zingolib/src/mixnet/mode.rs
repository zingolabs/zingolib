//! The five-state runtime state of the Nym mixnet (Mixnet Mode).
#![forbid(unsafe_code)]

use crate::mixnet::{DeathReport, MixnetProxy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Indicator {
    Unattached,
    SwitchedOff,
    Bootstrapping,
    Ready,
    PreviouslyProvenThisEpoch,
    Died,
}

impl Indicator {
    /// Every state, in declaration order. Exhaustive by construction: the
    /// wire round-trip tests iterate this array, so a new state that is not
    /// added here fails the exhaustiveness test rather than shipping
    /// untested.
    pub const ALL: [Indicator; 6] = [
        Indicator::Unattached,
        Indicator::SwitchedOff,
        Indicator::Bootstrapping,
        Indicator::Ready,
        Indicator::PreviouslyProvenThisEpoch,
        Indicator::Died,
    ];

    /// Whether a mixnet-only surface may proceed over the mixnet right
    /// now: true for earned [`Indicator::Ready`] and for stale-proven
    /// [`Indicator::PreviouslyProvenThisEpoch`], which routes the same.
    pub fn is_ready(self) -> bool {
        matches!(
            self,
            Indicator::Ready | Indicator::PreviouslyProvenThisEpoch
        )
    }

    /// Whether this mode is the recovery affordance's target: true exactly
    /// for [`Indicator::Died`], the one state that proves a transport was
    /// consented, established, and lost — where a re-enable repairs a loss.
    /// This is the session driver's recovery predicate (ADR 0024, decision
    /// 2), minted here so every consumer offers the affordance from one
    /// truth instead of re-deriving it.
    ///
    /// Deliberately false for [`Indicator::Unattached`]: the ground state
    /// carries no online intent — a wallet may never have consented to
    /// connectivity at all — and a failed enable reaches the consumer that
    /// expressed intent through the driver's typed error, not by reading
    /// intent into the mode. False for [`Indicator::SwitchedOff`] too:
    /// leaving it is consent revocation, a different act with different
    /// narration.
    pub fn needs_recovery(self) -> bool {
        matches!(self, Indicator::Died)
    }

    /// The canonical wire token for this state: the one mint every consumer
    /// renders from and parses back to (ADR 0024). The retired token `off`
    /// is deliberately not a token of any state — the 2026-07-28 amendment
    /// of ADR 0011 split it into `unattached` and `switched_off`, and a
    /// parser that still accepts it would resurrect the conflation.
    pub fn as_str(self) -> &'static str {
        match self {
            Indicator::Unattached => "unattached",
            Indicator::SwitchedOff => "switched_off",
            Indicator::Bootstrapping => "bootstrapping",
            Indicator::Ready => "ready",
            Indicator::PreviouslyProvenThisEpoch => "previously_proven_this_epoch",
            Indicator::Died => "died",
        }
    }
}

impl std::fmt::Display for Indicator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The token a [`Indicator`] parse rejected, carried whole so the consumer
/// can name it in its own narration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a Mixnet Mode wire token: {0:?}")]
pub struct UnknownIndicatorToken(pub String);

impl std::str::FromStr for Indicator {
    type Err = UnknownIndicatorToken;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Indicator::ALL
            .into_iter()
            .find(|mode| mode.as_str() == token)
            .ok_or_else(|| UnknownIndicatorToken(token.to_string()))
    }
}

/// The wallet's mixnet transport slot: the explicit state [`Indicator`] is
/// read from. An enum rather than `Option<MixnetProxy>` because dropping the
/// handle on disable would erase the very bit that separates
/// [`Indicator::SwitchedOff`] (a deliberate disable) from
/// [`Indicator::Unattached`] (absence of a transport) — the flattening the
/// 2026-07-28 amendment of ADR 0011 retires. Neither consents to nakednet:
/// since the 2026-09-11 amendment that is the transmit policy's answer,
/// held on the client beside this slot.
// One slot lives per client and never in a collection, so the size skew
// between the unit states and the attached transport costs nothing; boxing
// would add only indirection.
#[allow(clippy::large_enum_variant)]
pub(crate) enum MixnetSlot {
    /// No transport and no consent recorded. The initial state, and the
    /// state a failed enable leaves behind.
    Unattached,
    /// The user's deliberate per-session disable. Both route resolvers
    /// read it as a missing transport.
    SwitchedOff,
    /// The session's Standing Client, in whatever lifecycle state its
    /// transport reports.
    Attached(StandingClient),
    /// A stand-in transport for chain-mock tests: reports
    /// [`Indicator::Ready`] at the given address without a child, watcher,
    /// or probe, so the tests exercise the fail-closed route resolver and
    /// the escalation orchestration for real.
    #[cfg(any(test, feature = "testutils"))]
    AttachedForTests {
        /// The address the stand-in publishes into its status.
        socks5_addr: std::net::SocketAddr,
        /// The conduit the route resolver hands to Ready-mode surfaces.
        conduit: crate::mixnet::MixnetConduit,
        /// Whether sends submit over the mock indexer's channel instead of the
        /// SOCKS5 wire.
        mock_arms: bool,
    },
}

/// The session's one long-lived mixnet client: the transport every
/// operation but the price fetch multiplexes over, holding the lease of the
/// exit it was born proven on.
pub(crate) struct StandingClient {
    proxy: MixnetProxy,
    /// The bound exit's Reservation, recycled by drop; `None` for a
    /// mobile-attached endpoint, whose exit the host drew outside this
    /// session's Exit Pool.
    exit_reservation: Option<crate::mixnet::pools::exit_pool::Reservation>,
    /// Whether this client's birth answered the Sentinel itself; a
    /// trusting birth stands on a stale EpochProven observation instead.
    born_probed: bool,
    /// Whether a round trip of this client's own has confirmed the exit,
    /// which promotes stale proof to earned.
    confirmed: std::sync::atomic::AtomicBool,
    /// Whether the exit was convicted under this client, the state the
    /// failover's replacement birth runs in.
    condemned: std::sync::atomic::AtomicBool,
    /// Whether the failover exhausted every birth, the unconsented loss
    /// that latches Died until an explicit re-enable.
    forsaken: std::sync::Mutex<Option<DeathReport>>,
    /// The instant this client's proof stops being epoch-fresh, when a new
    /// ProofAcquisition is due.
    proof_deadline: std::sync::Mutex<std::time::Instant>,
    /// The one conduit every surface routes through, minted when the
    /// transport first announces its address and shared by clone from
    /// there, so a rotation supersedes what the whole session is using.
    conduit: std::sync::Mutex<Option<crate::mixnet::MixnetConduit>>,
}

impl StandingClient {
    /// A Standing Client over `proxy`, holding `exit_reservation` for its
    /// life, with `born_probed` recording whether its birth answered the
    /// Sentinel or trusted a stale EpochProven observation.
    pub(crate) fn new(
        proxy: MixnetProxy,
        exit_reservation: Option<crate::mixnet::pools::exit_pool::Reservation>,
        born_probed: bool,
    ) -> Self {
        StandingClient {
            proxy,
            exit_reservation,
            born_probed,
            confirmed: std::sync::atomic::AtomicBool::new(false),
            condemned: std::sync::atomic::AtomicBool::new(false),
            forsaken: std::sync::Mutex::new(None),
            proof_deadline: std::sync::Mutex::new(
                std::time::Instant::now() + zl_netutils::time::NYM_EPOCH,
            ),
            conduit: std::sync::Mutex::new(None),
        }
    }

    /// The client's conduit, minted on the first read that finds an
    /// announced address and the same conduit for every read after.
    pub(crate) fn conduit(&self) -> Option<crate::mixnet::MixnetConduit> {
        let mut held = self.conduit.lock().expect("conduit mutex");
        if held.is_none() {
            *held = self
                .proxy
                .socks5_addr()
                .map(crate::mixnet::MixnetConduit::over);
        }
        held.clone()
    }

    /// The client's transport.
    pub(crate) fn proxy(&self) -> &MixnetProxy {
        &self.proxy
    }

    /// Convicts the exit under this client, returning whether this call was
    /// the convicting one.
    pub(crate) fn condemn(&self) -> bool {
        !self
            .condemned
            .swap(true, std::sync::atomic::Ordering::AcqRel)
    }

    /// Whether the exit was convicted under this client.
    pub(crate) fn is_condemned(&self) -> bool {
        self.condemned.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Latches the failover's exhaustion — the unconsented loss of the
    /// transport — holding `cause` as the death's typed story.
    pub(crate) fn forsake(&self, cause: zl_netutils::diag::NetOpFailure) {
        *self.forsaken.lock().expect("forsaken mutex") = Some(DeathReport {
            at: std::time::SystemTime::now(),
            detail: Some(cause),
        });
    }

    /// The typed death this client latched: the forsaken exhaustion when
    /// one was ruled, otherwise whatever death its transport reports.
    pub(crate) fn death_report(&self) -> Option<DeathReport> {
        self.forsaken
            .lock()
            .expect("forsaken mutex")
            .clone()
            .or_else(|| self.proxy.death_report())
    }

    /// The instant this client's proof stops being epoch-fresh.
    pub(crate) fn proof_deadline(&self) -> std::time::Instant {
        *self.proof_deadline.lock().expect("proof deadline mutex")
    }

    /// Moves the proof deadline to `deadline`, after a fresh proof.
    pub(crate) fn set_proof_deadline(&self, deadline: std::time::Instant) {
        *self.proof_deadline.lock().expect("proof deadline mutex") = deadline;
    }

    /// The bound exit's identity, when this session's Exit Pool issued it.
    pub(crate) fn exit_node(&self) -> Option<&crate::mixnet::ExitNodeId> {
        self.exit_reservation
            .as_ref()
            .map(crate::mixnet::pools::exit_pool::Reservation::node)
    }

    /// Whether this client still stands on stale, unconfirmed proof.
    pub(crate) fn stale_unconfirmed(&self) -> bool {
        !self.born_probed && !self.confirmed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Records a completed round trip of this client's own — resetting the
    /// proof deadline one epoch out — and returns whether this call was the
    /// promoting one.
    pub(crate) fn note_round_trip(&self) -> bool {
        self.set_proof_deadline(std::time::Instant::now() + zl_netutils::time::NYM_EPOCH);
        !self
            .confirmed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
            && !self.born_probed
    }

    /// Stops the transport; dropping self recycles the reservation after.
    pub(crate) async fn stop(self) {
        self.proxy.stop().await;
    }

    /// Hands the session to a replacement: supersedes this client's conduit
    /// so no new work dials it, waits for the work already dialed to
    /// finish, then stops.
    pub(crate) async fn retire(self) {
        let Some(conduit) = self.conduit() else {
            // A client with no announced address carried nothing, so there
            // is nothing to drain.
            self.stop().await;
            return;
        };
        conduit.supersede();
        let deadline = std::time::Instant::now() + zl_netutils::time::CONDUIT_DRAIN_BUDGET;
        while conduit.state() != zl_netutils::conduit::ConduitState::Retired {
            if std::time::Instant::now() >= deadline {
                // A guard outliving the longest bounded operation is a leak,
                // not slow work: holding the transport open for it would
                // keep two clients alive for the rest of the session.
                tracing::warn!(
                    "a superseded conduit still had {} use(s) outstanding after its drain \
                     budget; stopping the transport regardless",
                    conduit.in_flight()
                );
                break;
            }
            tokio::time::sleep(zl_netutils::time::CONDUIT_DRAIN_POLL).await;
        }
        self.stop().await;
    }
}

impl MixnetSlot {
    /// The Mixnet Mode this slot is in: the slot's own state when no
    /// Standing Client is attached, otherwise the client's lifecycle state
    /// refined by its proof — forsaken latches Died, a condemned exit dips
    /// to Bootstrapping while the failover births a replacement, and stale
    /// unconfirmed proof is typed [`Indicator::PreviouslyProvenThisEpoch`]
    /// rather than earned Ready.
    pub(crate) fn mode(&self) -> Indicator {
        match self {
            MixnetSlot::Unattached => Indicator::Unattached,
            MixnetSlot::SwitchedOff => Indicator::SwitchedOff,
            MixnetSlot::Attached(client) => {
                if client.forsaken.lock().expect("forsaken mutex").is_some() {
                    return Indicator::Died;
                }
                if client.is_condemned() {
                    return Indicator::Bootstrapping;
                }
                match client.proxy().mode() {
                    Indicator::Ready if client.stale_unconfirmed() => {
                        Indicator::PreviouslyProvenThisEpoch
                    }
                    lifecycle => lifecycle,
                }
            }
            #[cfg(any(test, feature = "testutils"))]
            MixnetSlot::AttachedForTests { .. } => Indicator::Ready,
        }
    }

    /// The slot's typed death story, present exactly when a held client has
    /// latched one.
    pub(crate) fn death_report(&self) -> Option<DeathReport> {
        match self {
            MixnetSlot::Attached(client) => client.death_report(),
            _ => None,
        }
    }

    /// The Standing Client's transport, when one is attached.
    pub(crate) fn proxy(&self) -> Option<&MixnetProxy> {
        match self {
            MixnetSlot::Attached(client) => Some(client.proxy()),
            MixnetSlot::Unattached | MixnetSlot::SwitchedOff => None,
            #[cfg(any(test, feature = "testutils"))]
            MixnetSlot::AttachedForTests { .. } => None,
        }
    }

    /// The local SOCKS5 address a published status carries, wherever the
    /// slot keeps it: the Standing Client's announced address when one is
    /// attached, the pinned address of a test stand-in.
    pub(crate) fn socks5_addr(&self) -> Option<std::net::SocketAddr> {
        match self {
            MixnetSlot::Attached(client) => client.proxy().socks5_addr(),
            MixnetSlot::Unattached | MixnetSlot::SwitchedOff => None,
            #[cfg(any(test, feature = "testutils"))]
            MixnetSlot::AttachedForTests { socks5_addr, .. } => Some(*socks5_addr),
        }
    }

    /// The conduit the route resolver hands to Ready-mode surfaces, one per
    /// attached client so a rotation supersedes what the session is using.
    pub(crate) fn conduit(&self) -> Option<crate::mixnet::MixnetConduit> {
        match self {
            MixnetSlot::Attached(client) => client.conduit(),
            MixnetSlot::Unattached | MixnetSlot::SwitchedOff => None,
            #[cfg(any(test, feature = "testutils"))]
            MixnetSlot::AttachedForTests { conduit, .. } => Some(conduit.clone()),
        }
    }

    /// The bound Exit Node identities, when the Standing Client is ready.
    pub(crate) fn exits(&self) -> Vec<crate::mixnet::ExitNodeId> {
        match self {
            MixnetSlot::Attached(client) => client.proxy().exits(),
            MixnetSlot::Unattached | MixnetSlot::SwitchedOff => Vec::new(),
            #[cfg(any(test, feature = "testutils"))]
            MixnetSlot::AttachedForTests { .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
mod wire_contract;

#[cfg(test)]
mod stale_proof;

#[cfg(test)]
mod hand_off;

#[cfg(test)]
mod demotion;
