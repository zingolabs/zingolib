//! The Exit Pool and the Proven Client acquisition every operation shares.
#![forbid(unsafe_code)]

pub(crate) mod exit_pool;

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
}

impl Pools {
    /// An empty exit authority for a fresh session.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Pools {
            exits: std::sync::Arc::new(std::sync::Mutex::new(exit_pool::ExitPool::default())),
            acquirer: std::sync::Mutex::new(None),
        })
    }

    /// Draws a Clutch, seeding the Exit Pool from the directory first when
    /// this session has not yet learned the population.
    pub(crate) async fn draw_clutch(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    ) -> Result<std::collections::HashSet<exit_pool::Reservation>, acquire::TransportError> {
        let seeded = self.exits.lock().expect("exit pool mutex").is_seeded();
        if !seeded {
            let discovered = acquirer.discover().await?;
            self.exits.lock().expect("exit pool mutex").seed(discovered);
        }
        Ok(exit_pool::ExitPool::draw_clutch(&self.exits)?)
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

    /// The instant `exit`'s EpochProven observation stops being fresh,
    /// when one stands.
    pub(crate) fn proven_until(
        &self,
        exit: &crate::mixnet::ExitNodeId,
    ) -> Option<std::time::Instant> {
        self.exits
            .lock()
            .expect("exit pool mutex")
            .proven_until(exit)
    }

    /// Keeps `verdict` as `exit`'s current observation, earned now.
    pub(crate) fn remember(
        &self,
        exit: crate::mixnet::ExitNodeId,
        verdict: exit_pool::ExitNodeHealthVerdict,
    ) {
        self.exits.lock().expect("exit pool mutex").remember(
            exit,
            exit_pool::Observation::earned(verdict, std::time::Instant::now()),
        );
    }

    /// Acquires one ready transport over a fresh Clutch, publishing its
    /// lifecycle into `publisher` and keeping only the bound exit's lease;
    /// a bind failure names the exits the failed Clutch drew, so the birth
    /// loop can convict them.
    async fn acquire_bound(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
        publisher: &crate::mixnet::driver::StatusPublisher,
    ) -> Result<(crate::mixnet::MixnetProxy, exit_pool::Reservation), BindFailure> {
        let mut clutch = self
            .draw_clutch(acquirer)
            .await
            .map_err(BindFailure::undrawn)?;
        let nodes = exit_pool::clutch_nodes(&clutch);
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
        // Bind-time recycle: keeping only the bound lease drops the rest. A
        // report naming no drawn node (a defective host or child) refuses
        // typed, with the transport stopped and every reservation recycled.
        let Some(lease) = exit_pool::take_bound_lease(&mut clutch, &exits) else {
            transport.stop().await;
            return Err(BindFailure {
                cause: acquire::TransportError::ExitOutsideClutch { reported: exits },
                drawn: nodes,
            });
        };
        drop(clutch);
        Ok((transport, lease))
    }

    /// Births one Proven Client — a trusting birth when the bound exit's
    /// fresh proof is trusted, otherwise a proving birth whose Sentinel
    /// refusal condemns the exit and tries a successor, refusing typed when
    /// every birth failed its proof — returning `probed: false` for a
    /// trusting birth so the caller can type the stale proof. A bind-stage
    /// failure spends a birth too: the drawn exits are convicted and a
    /// fresh Clutch drawn, so one slow or dead Clutch is absorbed by the
    /// budget instead of aborting the whole acquisition with nothing
    /// learned.
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
            let trusted = {
                let exits = self.exits.lock().expect("exit pool mutex");
                exits.epoch_proven(lease.node(), std::time::Instant::now())
            };
            if trusted {
                return Ok(ProvenBirth {
                    transport,
                    lease,
                    probed: false,
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
            if evidence.proves_the_exit() {
                self.remember(
                    lease.node().clone(),
                    exit_pool::ExitNodeHealthVerdict::EpochProven,
                );
                return Ok(ProvenBirth {
                    transport,
                    lease,
                    probed: true,
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
pub(crate) struct ProvenBirth {
    /// The ready transport.
    pub(crate) transport: crate::mixnet::MixnetProxy,
    /// The bound exit's lease.
    pub(crate) lease: exit_pool::Reservation,
    /// Whether this birth answered the Sentinel itself.
    pub(crate) probed: bool,
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
            let now = std::time::Instant::now();
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
