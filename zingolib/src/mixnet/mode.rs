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
/// [`Indicator::SwitchedOff`] (consent to clearnet) from
/// [`Indicator::Unattached`] (absence of a transport) — the flattening the
/// 2026-07-28 amendment of ADR 0011 retires.
// One slot lives per client and never in a collection, so the size skew
// between the unit states and the attached transport costs nothing; boxing
// would add only indirection.
#[allow(clippy::large_enum_variant)]
pub(crate) enum MixnetSlot {
    /// No transport and no consent recorded. The initial state, and the
    /// state a failed enable leaves behind.
    Unattached,
    /// The user's deliberate per-session disable. The one slot state that
    /// consents to clearnet.
    SwitchedOff,
    /// The session's Standing Client, in whatever lifecycle state its
    /// transport reports.
    Attached(StandingClient),
    /// A stand-in transport for chain-mock tests: reports
    /// [`Indicator::Ready`] at the given address without a child, watcher,
    /// or probe, so the tests exercise the fail-closed route resolver and
    /// the escalation orchestration for real. Only
    /// `LightClient::switch_on_mixnet_for_tests` constructs it, and the
    /// transmit path pairs it with arms that submit over the mock indexer's
    /// channel — the address is never dialed.
    #[cfg(any(test, feature = "testutils"))]
    AttachedForTests {
        /// The address the stand-in publishes into its status.
        socks5_addr: std::net::SocketAddr,
        /// The conduit the route resolver hands to Ready-mode surfaces.
        conduit: crate::mixnet::MixnetConduit,
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
    exit_reservation: Option<crate::correspondent::pool::exit_pool::Reservation>,
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
        exit_reservation: Option<crate::correspondent::pool::exit_pool::Reservation>,
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
                std::time::Instant::now() + zingo_netutils::time::NYM_EPOCH,
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
    pub(crate) fn forsake(&self, cause: zingo_net_diag::NetOpFailure) {
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
            .map(crate::correspondent::pool::exit_pool::Reservation::node)
    }

    /// Whether this client still stands on stale, unconfirmed proof.
    pub(crate) fn stale_unconfirmed(&self) -> bool {
        !self.born_probed && !self.confirmed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Records a completed round trip of this client's own — resetting the
    /// proof deadline one epoch out — and returns whether this call was the
    /// promoting one.
    pub(crate) fn note_round_trip(&self) -> bool {
        self.set_proof_deadline(std::time::Instant::now() + zingo_netutils::time::NYM_EPOCH);
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
        let deadline = std::time::Instant::now() + zingo_netutils::time::CONDUIT_DRAIN_BUDGET;
        while conduit.state() != zingo_netutils::conduit::ConduitState::Retired {
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
            tokio::time::sleep(zingo_netutils::time::CONDUIT_DRAIN_POLL).await;
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
mod wire_contract {
    use std::str::FromStr as _;

    use super::Indicator;

    /// The ratified tokens, pinned literally so a rename in `as_str` cannot
    /// pass silently: this list is the wire contract of ADR 0024.
    const RATIFIED_TOKENS: [(Indicator, &str); 6] = [
        (Indicator::Unattached, "unattached"),
        (Indicator::SwitchedOff, "switched_off"),
        (Indicator::Bootstrapping, "bootstrapping"),
        (Indicator::Ready, "ready"),
        (
            Indicator::PreviouslyProvenThisEpoch,
            "previously_proven_this_epoch",
        ),
        (Indicator::Died, "died"),
    ];

    #[test]
    fn every_state_mints_its_ratified_token() {
        for (mode, token) in RATIFIED_TOKENS {
            assert_eq!(mode.as_str(), token);
            assert_eq!(mode.to_string(), token);
        }
    }

    #[test]
    fn serde_and_as_str_agree_on_every_state() {
        for mode in Indicator::ALL {
            let json = serde_json::to_string(&mode).expect("serialization is infallible");
            assert_eq!(json, format!("{:?}", mode.as_str()));
            let back: Indicator =
                serde_json::from_str(&json).expect("the minted token parses back");
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn from_str_round_trips_every_state() {
        for mode in Indicator::ALL {
            assert_eq!(Indicator::from_str(mode.as_str()), Ok(mode));
        }
    }

    #[test]
    fn the_retired_off_token_is_rejected() {
        // "off" is the conflation the five-state decomposition retired; a
        // parser that accepts it would quietly reunify consent with absence.
        assert!(Indicator::from_str("off").is_err());
        assert!(serde_json::from_str::<Indicator>("\"off\"").is_err());
    }

    /// HYPOTHESIS: a status in stale-proven mode carries its address and
    /// exits across the wire whole, because the mode routes exactly as
    /// Ready. Falsified if the guard rejects the evidence.
    #[test]
    fn a_stale_proven_status_round_trips_with_its_evidence() {
        let status = crate::mixnet::MixnetStatus {
            mode: Indicator::PreviouslyProvenThisEpoch,
            socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
            exits: vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
            bootstrap_detail: None,
            death: None,
        };
        let json = serde_json::to_string(&status).expect("serialization is infallible");
        let back: crate::mixnet::MixnetStatus =
            serde_json::from_str(&json).expect("a published status parses back");
        assert_eq!(back.mode, Indicator::PreviouslyProvenThisEpoch);
        assert_eq!(back.socks5_addr, status.socks5_addr);
        assert_eq!(back.exits, status.exits);
    }

    /// HYPOTHESIS: whatever raw fields a slot holds, the evidenced
    /// constructor emits a status every mode round-trips, so the publisher
    /// and the wire guard agree by construction. Falsified if any mode's
    /// published shape fails to deserialize.
    #[test]
    fn every_published_shape_round_trips() {
        for mode in Indicator::ALL {
            let published = crate::mixnet::MixnetStatus::evidenced(
                mode,
                Some("127.0.0.1:1080".parse().expect("the test address parses")),
                vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
                Some(crate::mixnet::DeathReport {
                    at: std::time::SystemTime::UNIX_EPOCH,
                    detail: None,
                }),
            );
            let json = serde_json::to_string(&published).expect("serialization is infallible");
            let back: Result<crate::mixnet::MixnetStatus, _> = serde_json::from_str(&json);
            assert!(
                back.is_ok(),
                "mode {} publishes a shape its own wire refuses: {json}",
                mode.as_str()
            );
        }
    }

    #[test]
    fn all_is_exhaustive() {
        // A new variant must join ALL: this match goes non-exhaustive the
        // moment one is added, and ALL's length is pinned by its type.
        for mode in Indicator::ALL {
            match mode {
                Indicator::Unattached
                | Indicator::SwitchedOff
                | Indicator::Bootstrapping
                | Indicator::Ready
                | Indicator::PreviouslyProvenThisEpoch
                | Indicator::Died => {}
            }
        }
    }
}

#[cfg(test)]
mod stale_proof {
    use super::{Indicator, MixnetSlot, StandingClient};

    /// HYPOTHESIS: a Standing Client born on stale proof reports
    /// PreviouslyProvenThisEpoch until a round trip of its own confirms
    /// the exit, and Ready after — falsified if stale proof masquerades
    /// as earned Ready, the trust hazard of a trusting birth.
    #[tokio::test]
    async fn stale_proof_is_typed_until_a_round_trip_confirms_it() {
        let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
            "127.0.0.1:1080".parse().expect("the test address parses"),
            vec![crate::mixnet::ExitNodeId::from("exit-stale-proven")],
        );
        let slot = MixnetSlot::Attached(StandingClient::new(proxy, None, false));

        assert_eq!(
            slot.mode(),
            Indicator::PreviouslyProvenThisEpoch,
            "stale proof must not masquerade as earned Ready"
        );

        if let MixnetSlot::Attached(client) = &slot {
            assert!(
                client.note_round_trip(),
                "the first confirmed round trip is the promoting one"
            );
        }
        assert_eq!(
            slot.mode(),
            Indicator::Ready,
            "a confirmed round trip promotes stale proof to earned"
        );
    }

    /// HYPOTHESIS: a Standing Client whose birth answered the Sentinel
    /// reports earned Ready from its first instant — falsified if a probed
    /// birth is typed stale.
    #[tokio::test]
    async fn a_probed_birth_is_ready_from_its_first_instant() {
        let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
            "127.0.0.1:1080".parse().expect("the test address parses"),
            vec![crate::mixnet::ExitNodeId::from("exit-earned")],
        );
        let slot = MixnetSlot::Attached(StandingClient::new(proxy, None, true));
        assert_eq!(slot.mode(), Indicator::Ready);
    }
}

#[cfg(test)]
mod hand_off {
    use super::StandingClient;
    use zingo_netutils::conduit::ConduitState;

    /// How many recheck intervals a held guard is observed across before
    /// the wait is called a wait rather than a scheduling accident.
    const DRAIN_OBSERVATION_POLLS: u32 = 3;

    fn client() -> StandingClient {
        let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
            "127.0.0.1:1080".parse().expect("the test address parses"),
            vec![crate::mixnet::ExitNodeId::from("exit-handed-off")],
        );
        StandingClient::new(proxy, None, true)
    }

    /// HYPOTHESIS: a client mints one conduit and hands the same one to
    /// every reader, so superseding it reaches work the session already
    /// routed. Falsified if two reads yield conduits with separate counts.
    #[tokio::test]
    async fn a_client_hands_every_reader_the_same_conduit() {
        let client = client();
        let first = client.conduit().expect("a ready client mints its conduit");
        let second = client.conduit().expect("the conduit is minted once");
        let held = first.dial();
        assert_eq!(
            second.in_flight(),
            1,
            "a second reader must see the first reader's use"
        );
        drop(held);
        assert_eq!(second.in_flight(), 0);
    }

    /// HYPOTHESIS: retirement holds the transport open for work already
    /// dialed and stops the moment that work ends, so ADR 0048's hand-off
    /// cuts no send. Falsified if the transport stops with a guard
    /// outstanding.
    #[tokio::test]
    async fn a_retiring_client_waits_for_its_outstanding_work() {
        let client = client();
        let conduit = client.conduit().expect("a ready client mints its conduit");
        let held = conduit.dial();

        let mut retiring = tokio::spawn(client.retire());
        assert!(
            tokio::time::timeout(
                zingo_netutils::time::CONDUIT_DRAIN_POLL * DRAIN_OBSERVATION_POLLS,
                &mut retiring,
            )
            .await
            .is_err(),
            "a hand-off must not stop the transport under work in flight"
        );
        assert_eq!(
            conduit.state(),
            ConduitState::Superseded,
            "the outgoing conduit takes no new work while it drains"
        );

        drop(held);
        tokio::time::timeout(zingo_netutils::time::CONDUIT_DRAIN_BUDGET, retiring)
            .await
            .expect("a drained conduit lets its transport stop")
            .expect("the retirement runs to completion");
        assert_eq!(conduit.state(), ConduitState::Retired);
    }

    /// HYPOTHESIS: a guard that outlives the drain budget does not hold the
    /// transport open forever, because a leaked guard would leave two
    /// clients alive for the rest of the session. Falsified if retirement
    /// never returns.
    #[tokio::test(start_paused = true)]
    async fn a_leaked_guard_does_not_strand_the_transport() {
        let client = client();
        let conduit = client.conduit().expect("a ready client mints its conduit");
        let _leaked = conduit.dial();
        client.retire().await;
        assert_eq!(
            conduit.state(),
            ConduitState::Superseded,
            "the budget expired rather than the work finishing"
        );
    }
}

#[cfg(test)]
mod demotion {
    use super::{Indicator, MixnetSlot, StandingClient};

    fn attached(born_probed: bool) -> MixnetSlot {
        let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
            "127.0.0.1:1080".parse().expect("the test address parses"),
            vec![crate::mixnet::ExitNodeId::from("exit-under-trial")],
        );
        MixnetSlot::Attached(StandingClient::new(proxy, None, born_probed))
    }

    /// HYPOTHESIS: convicting the exit under a live client dips the mode to
    /// Bootstrapping — the ruled state while the failover's replacement
    /// births — and never to Died, which names only the transport's own
    /// loss. Falsified if a condemned client still claims readiness.
    #[tokio::test]
    async fn a_condemned_client_dips_to_bootstrapping() {
        let slot = attached(true);
        assert_eq!(slot.mode(), Indicator::Ready);
        if let MixnetSlot::Attached(client) = &slot {
            assert!(client.condemn(), "the first conviction reports itself");
            assert!(!client.condemn(), "a second conviction is idempotent");
        }
        assert_eq!(
            slot.mode(),
            Indicator::Bootstrapping,
            "a convicted exit means the session cannot truthfully claim readiness"
        );
    }

    /// HYPOTHESIS: a failover that exhausts every birth latches Died — the
    /// unconsented loss — which only an explicit re-enable leaves.
    /// Falsified if exhaustion reads as anything softer.
    #[tokio::test]
    async fn an_exhausted_failover_latches_died() {
        let slot = attached(true);
        if let MixnetSlot::Attached(client) = &slot {
            client.condemn();
            client.forsake(zingo_net_diag::NetOpFailure::message(
                zingo_net_diag::NetOpStage::RouteResolution,
                "the session's exit census",
                "every failover birth failed its proof",
            ));
        }
        assert_eq!(slot.mode(), Indicator::Died);
        assert!(
            slot.death_report()
                .is_some_and(|death| death.detail.is_some()),
            "the latched death holds its typed story"
        );
    }
}
