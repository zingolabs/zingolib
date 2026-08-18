//! The Exit Pool and the Proven Client acquisition every operation shares.
#![forbid(unsafe_code)]

pub(crate) mod exit_pool;

use std::time::{Duration, Instant};

use crate::mixnet::acquire;

/// How many proving births one acquisition may attempt before it refuses,
/// bounded so a mixnet failing everywhere refuses in a stated time rather
/// than birthing forever.
pub(crate) const MAX_PROVING_BIRTHS: usize = 6;

/// The transport surface a member needs; production implements it on the
/// spawned `MixnetProxy`.
pub(crate) trait PoolTransport: Send + 'static {
    /// The transport's local SOCKS5 address, while it lives.
    fn socks5_addr(&self) -> Option<std::net::SocketAddr>;
    /// Tears the transport down.
    fn stop(self) -> impl std::future::Future<Output = ()> + Send;
}

/// One ready client: a transport with a bound exit and the owning lease of
/// the Exit Node it bound.
pub(crate) struct Member<T> {
    transport: T,
    lease: exit_pool::Reservation,
}

impl<T: PoolTransport> Member<T> {
    /// A member over `transport`, holding `lease` for its life.
    pub(crate) fn new(transport: T, lease: exit_pool::Reservation) -> Self {
        Member { transport, lease }
    }

    /// The bound Exit Node's identity.
    pub(crate) fn node(&self) -> &crate::mixnet::ExitNodeId {
        self.lease.node()
    }

    /// Stops the transport and recycles the lease.
    pub(crate) async fn retire(self) {
        self.transport.stop().await;
    }

    /// The tunnel address for one more contact, while the transport lives.
    pub(crate) fn addr(&self) -> Option<std::net::SocketAddr> {
        self.transport.socks5_addr()
    }
}

/// The session's exit authority: the Exit Pool of Reservations, the
/// NodeHealthIndex behind its draws, and the acquirer Proven Clients are
/// born from.
pub(crate) struct Pools {
    /// The session's sole issuer of Exit Node Reservations; reservations
    /// hold a weak ledger handle and recycle themselves on drop.
    pub(crate) exits: std::sync::Arc<std::sync::Mutex<exit_pool::ExitPool>>,
    /// What births acquire transports from; `None` until a session sets
    /// one, and always `None` for attached sessions.
    acquirer:
        std::sync::Mutex<Option<std::sync::Arc<dyn crate::mixnet::acquire::TransportAcquirable>>>,
    /// Which exit holds which role, recorded when boot proves its quartet.
    /// The binding outlives every client, so the next client doing a job
    /// binds the exit that already carries that job's role (ADR 0045).
    roles: std::sync::Mutex<
        std::collections::HashMap<crate::mixnet::quartet::Role, crate::mixnet::ExitNodeId>,
    >,
}

impl Pools {
    /// An empty exit authority for a fresh session.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Pools {
            exits: std::sync::Arc::new(std::sync::Mutex::new(exit_pool::ExitPool::default())),
            acquirer: std::sync::Mutex::new(None),
            roles: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Draws one exit reservation, seeding the Exit Pool from the directory
    /// first when this session has not yet learned the population.
    pub(crate) async fn draw_exit(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    ) -> Result<exit_pool::Reservation, acquire::TransportError> {
        let seeded = self.exits.lock().expect("exit pool mutex").is_seeded();
        if !seeded {
            let discovered = acquirer.discover().await?;
            self.exits.lock().expect("exit pool mutex").seed(discovered);
        }
        Ok(exit_pool::ExitPool::draw_exit(&self.exits)?)
    }

    /// Records what this session acquires transports from.
    pub(crate) fn set_acquirer(
        &self,
        acquirer: std::sync::Arc<dyn crate::mixnet::acquire::TransportAcquirable>,
    ) {
        *self.acquirer.lock().expect("pool acquirer mutex") = Some(acquirer);
    }

    /// Forgets the acquirer at session teardown, so nothing births against
    /// a torn-down session.
    pub(crate) fn clear_acquirer(&self) {
        *self.acquirer.lock().expect("pool acquirer mutex") = None;
    }

    /// This session's acquirer, when one is set.
    pub(crate) fn acquirer(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::mixnet::acquire::TransportAcquirable>> {
        self.acquirer.lock().expect("pool acquirer mutex").clone()
    }

    /// Records that `exit` holds `role` for as long as its proof stands.
    pub(crate) fn assign_role(
        &self,
        role: crate::mixnet::quartet::Role,
        exit: crate::mixnet::ExitNodeId,
    ) {
        self.roles
            .lock()
            .expect("role binding mutex")
            .insert(role, exit);
    }

    /// Keeps `verdict` as `exit`'s current observation, earned now.
    pub(crate) fn remember(
        &self,
        exit: crate::mixnet::ExitNodeId,
        verdict: exit_pool::ExitNodeHealthVerdict,
    ) {
        self.exits.lock().expect("exit pool mutex").remember(
            exit,
            exit_pool::Observation::earned(verdict, Instant::now()),
        );
    }

    /// Acquires one ready transport over a freshly drawn exit, publishing
    /// its lifecycle into `publisher`; a bind failure names the exit it drew,
    /// so the birth loop can convict it.
    async fn acquire_bound(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
        publisher: &crate::mixnet::driver::StatusPublisher,
    ) -> Result<(crate::mixnet::MixnetProxy, exit_pool::Reservation), BindFailure> {
        let lease = self
            .draw_exit(acquirer)
            .await
            .map_err(BindFailure::undrawn)?;
        let nodes = vec![lease.node().clone()];
        let (transport, exits) = match crate::mixnet::supervisor::acquire_ready_transport(
            acquirer,
            &nodes,
            std::sync::Arc::clone(publisher),
        )
        .await
        {
            Ok(bound) => bound,
            Err(cause) => {
                return Err(BindFailure {
                    cause,
                    drawn: nodes,
                });
            }
        };
        // The birth drew one exit, so a report naming any other is a
        // defective host or child: refuse typed, stop the transport, and let
        // the reservation recycle on drop.
        if !exits.contains(lease.node()) {
            transport.stop().await;
            return Err(BindFailure {
                cause: acquire::TransportError::ExitOutsideClutch { reported: exits },
                drawn: nodes,
            });
        }
        Ok((transport, lease))
    }

    /// Births one Proven Client, carrying a [`Proof::Inherited`] when the
    /// bound exit's fresh observation is trusted and a [`Proof::Earned`]
    /// when the Sentinel answered, condemning the exit and trying a
    /// successor when it does not, and refusing typed when every birth
    /// failed its proof. A bind-stage failure spends a birth too: the drawn
    /// exit is convicted and a fresh one drawn, so one slow or dead exit is
    /// absorbed by the budget instead of aborting the whole acquisition
    /// with nothing learned.
    pub(crate) async fn acquire_proven(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    ) -> Result<ProvenBirth, acquire::TransportError> {
        // The birth channel is minted here, never taken from the caller, so
        // no candidate lifecycle can reach the session's subscribers: the
        // returned birth carries the channel, and the slot owner alone
        // decides what the session hears.
        let lifecycle = crate::mixnet::status_publisher();
        for _birth in 0..MAX_PROVING_BIRTHS {
            let (transport, lease) = match self.acquire_bound(acquirer, &lifecycle).await {
                Ok(bound) => bound,
                Err(failure) if failure.retryable() => {
                    // The whole drawn Clutch produced no ready bound
                    // transport within budget: convict its exits so the
                    // next draw learns — unless the child's own report was
                    // defective — and spend a birth on it.
                    log::warn!("a birth failed at the bind stage: {}", failure.cause);
                    if failure.condemns_drawn() {
                        for node in failure.drawn {
                            self.remember(node, exit_pool::ExitNodeHealthVerdict::Failed);
                        }
                    }
                    continue;
                }
                Err(failure) => return Err(failure.cause),
            };
            // A trusted exit's own observation carries the deadline: the
            // birth inherits it rather than starting an epoch it never
            // earned, so the watchdog fires when the original proof ages out.
            let inherited = {
                let exits = self.exits.lock().expect("exit pool mutex");
                exits
                    .epoch_proven(lease.node(), Instant::now())
                    .then(|| exits.proven_until(lease.node()))
                    .flatten()
            };
            if let Some(proven_until) = inherited {
                return Ok(ProvenBirth {
                    transport,
                    lease,
                    proof: Proof::Inherited { proven_until },
                    lifecycle,
                });
            }
            let Some(socks5) = transport.socks5_addr() else {
                transport.stop().await;
                return Err(acquire::TransportError::DiedBeforeUse);
            };
            let evidence = zingo_netutils::sentinel::probe_sentinel(
                socks5,
                zingo_netutils::time::SENTINEL_BUDGET,
            )
            .await;
            if let zingo_netutils::sentinel::ExitEvidence::Answered { millis } = evidence {
                self.remember(
                    lease.node().clone(),
                    exit_pool::ExitNodeHealthVerdict::EpochProven,
                );
                return Ok(ProvenBirth {
                    transport,
                    lease,
                    proof: Proof::Earned {
                        round_trip: Duration::from_millis(millis),
                    },
                    lifecycle,
                });
            }
            self.remember(
                lease.node().clone(),
                exit_pool::ExitNodeHealthVerdict::Failed,
            );
            transport.stop().await;
        }
        Err(acquire::TransportError::NoProvenExit {
            probed: MAX_PROVING_BIRTHS,
            budget: zingo_netutils::time::SENTINEL_BUDGET,
        })
    }
}

/// One failed bind: its typed cause, and the exits the failed Clutch drew.
struct BindFailure {
    /// The typed failure the bind stage produced.
    cause: acquire::TransportError,
    /// The drawn exits, none of which produced a ready bound transport.
    drawn: Vec<crate::mixnet::ExitNodeId>,
}

impl BindFailure {
    /// A failure from before any Clutch was drawn, which no retry helps.
    fn undrawn(cause: acquire::TransportError) -> Self {
        BindFailure {
            cause,
            drawn: Vec::new(),
        }
    }

    /// Whether spending another birth can help: true for the failures of a
    /// drawn Clutch that would not repeat on fresh exits, false for the
    /// environment's own refusals — a missing binary, an unreachable host,
    /// an unseeded or exhausted pool.
    fn retryable(&self) -> bool {
        match &self.cause {
            acquire::TransportError::NotReady { .. }
            | acquire::TransportError::DiedDuringBootstrap { .. }
            | acquire::TransportError::StatusChannelClosed
            | acquire::TransportError::ExitOutsideClutch { .. } => !self.drawn.is_empty(),
            _ => false,
        }
    }

    /// Whether the failure indicts the drawn exits themselves: a Clutch
    /// that never became ready condemns its exits, while a defective exit
    /// report indicts the child, not the exits it failed to name.
    fn condemns_drawn(&self) -> bool {
        !matches!(
            self.cause,
            acquire::TransportError::ExitOutsideClutch { .. }
        )
    }
}

/// One Proven Client's birth: the transport, its bound exit's lease, and
/// whether the proof was earned by this birth's own Sentinel answer rather
/// than trusted from a stale EpochProven observation.
/// What a birth's proof rests on, which decides how long the client may be
/// trusted before a fresh ProofAcquisition is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Proof {
    /// The birth answered the Sentinel itself, so its exit is proven for a
    /// whole epoch from now.
    Earned {
        /// How long the Sentinel's round trip took.
        round_trip: Duration,
    },
    /// The birth trusted a fresh `EpochProven` observation an earlier client
    /// earned, so it inherits that observation's expiry rather than starting
    /// an epoch of its own.
    Inherited {
        /// When the inherited observation stops being epoch-fresh.
        proven_until: Instant,
    },
}

pub(crate) struct ProvenBirth {
    /// The ready transport.
    pub(crate) transport: crate::mixnet::MixnetProxy,
    /// The bound exit's lease.
    pub(crate) lease: exit_pool::Reservation,
    /// What this birth's proof rests on.
    pub(crate) proof: Proof,
    /// The birth's own status channel, carrying the condemned candidates'
    /// churn and the settled client's later transitions.
    pub(crate) lifecycle: crate::mixnet::driver::StatusPublisher,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransport {
        ready: bool,
    }

    impl PoolTransport for FakeTransport {
        fn socks5_addr(&self) -> Option<std::net::SocketAddr> {
            self.ready
                .then(|| "127.0.0.1:7".parse().expect("the fake address parses"))
        }

        fn stop(self) -> impl std::future::Future<Output = ()> + Send {
            std::future::ready(())
        }
    }

    /// HYPOTHESIS: a member serves repeated dials for its one holder, then
    /// retires once.
    #[tokio::test]
    async fn a_member_serves_repeated_dials() {
        let member: Member<FakeTransport> = Member::new(
            FakeTransport { ready: true },
            exit_pool::Reservation::dangling_for_test("exit-shared"),
        );
        let first = member.addr().expect("a live shared member has an address");
        let second = member.addr().expect("a second dial is permitted");
        assert_eq!(first, second, "one tunnel serves the whole fan-out");
        member.retire().await;
    }

    /// HYPOTHESIS: clearing the acquirer forbids later births, the teardown
    /// property vacating a session relies on.
    #[test]
    fn clearing_the_acquirer_forbids_later_births() {
        let pools = Pools::new();
        assert!(
            pools.acquirer().is_none(),
            "a fresh session has no acquirer"
        );
        pools.clear_acquirer();
        assert!(pools.acquirer().is_none(), "clearing is idempotent");
    }
}

#[cfg(test)]
mod bind_failure_absorption {
    //! Finding F2's contract: a bind-stage failure spends a birth and
    //! convicts the drawn exits instead of escaping the six-birth loop.

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// An acquirer whose every acquisition fails at the bind stage, with a
    /// census large enough that no draw repeats an exit.
    struct BindRefusingAcquirer {
        census: usize,
        acquisitions: AtomicUsize,
    }

    impl crate::mixnet::acquire::TransportAcquirable for BindRefusingAcquirer {
        fn discover(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            std::collections::HashSet<crate::mixnet::ExitNodeId>,
                            acquire::TransportError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            let census = self.census;
            Box::pin(async move {
                Ok((0..census)
                    .map(|index| crate::mixnet::ExitNodeId::from(format!("exit-{index}").as_str()))
                    .collect())
            })
        }

        fn acquire<'a>(
            &'a self,
            _clutch: &'a [crate::mixnet::ExitNodeId],
            _publisher: crate::mixnet::driver::StatusPublisher,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<crate::mixnet::MixnetProxy, acquire::TransportError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.acquisitions.fetch_add(1, Ordering::AcqRel);
            Box::pin(async {
                Err(acquire::TransportError::NotReady {
                    budget: zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
                })
            })
        }
    }

    /// HYPOTHESIS: a bind-stage failure spends one of the six births and
    /// convicts every drawn exit, so the loop absorbs it and the final
    /// refusal is the typed NoProvenExit — never the first bind error
    /// escaping with nothing learned. Falsified if the loop exits early or
    /// the drawn-and-dead exits stay eligible.
    #[tokio::test]
    async fn a_bind_failure_spends_a_birth_and_convicts_the_drawn() {
        let pools = Pools::new();
        let acquirer = BindRefusingAcquirer {
            census: MAX_PROVING_BIRTHS * zingo_netutils::arm_race::RESERVATION_CLUTCH_SIZE,
            acquisitions: AtomicUsize::new(0),
        };
        let Err(refusal) = pools.acquire_proven(&acquirer).await else {
            panic!("every bind fails, so no birth can succeed");
        };

        assert!(
            matches!(
                refusal,
                acquire::TransportError::NoProvenExit { probed, .. }
                    if probed == MAX_PROVING_BIRTHS
            ),
            "the loop absorbs bind failures into its budget, got: {refusal}"
        );
        assert_eq!(
            acquirer.acquisitions.load(Ordering::Acquire),
            MAX_PROVING_BIRTHS,
            "every birth was spent on an acquisition"
        );
        let convicted = {
            let exits = pools.exits.lock().expect("exit pool mutex");
            let now = Instant::now();
            (0..acquirer.census)
                .filter(|index| {
                    exits.epoch_failed(
                        &crate::mixnet::ExitNodeId::from(format!("exit-{index}").as_str()),
                        now,
                    )
                })
                .count()
        };
        assert_eq!(
            convicted, acquirer.census,
            "every drawn-and-dead exit is convicted rather than staying eligible"
        );
    }
}
