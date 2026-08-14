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

/// The sealed category of a bound exit's Correspondent exposure.
pub(crate) trait ExitUse: sealed::Sealed + Send + 'static {}

/// The category fanning out to many Correspondents for its one holder.
pub(crate) enum Shared {}

mod sealed {
    pub(crate) trait Sealed {}
    impl Sealed for super::Shared {}
}

impl ExitUse for Shared {}

/// One ready client: a transport with a bound exit and the owning lease of
/// the Exit Node it bound, in the exit-use category `U`.
pub(crate) struct Member<T, U: ExitUse> {
    transport: T,
    lease: exit_pool::Reservation,
    category: std::marker::PhantomData<U>,
}

impl<T: PoolTransport, U: ExitUse> Member<T, U> {
    /// A member in the exit-use category its acquisition site declares.
    pub(crate) fn new(transport: T, lease: exit_pool::Reservation) -> Self {
        Member {
            transport,
            lease,
            category: std::marker::PhantomData,
        }
    }

    /// The bound Exit Node's identity.
    pub(crate) fn node(&self) -> &crate::mixnet::ExitNodeId {
        self.lease.node()
    }

    /// Stops the transport and recycles the lease.
    pub(crate) async fn retire(self) {
        self.transport.stop().await;
    }
}

impl<T: PoolTransport> Member<T, Shared> {
    /// The tunnel address for one more Correspondent contact, while the
    /// transport lives.
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
    /// lifecycle into `publisher` and keeping only the bound exit's lease.
    pub(crate) async fn acquire_bound(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
        publisher: &crate::mixnet::driver::StatusPublisher,
    ) -> Result<(crate::mixnet::MixnetProxy, exit_pool::Reservation), acquire::TransportError> {
        let mut clutch = self.draw_clutch(acquirer).await?;
        let nodes = exit_pool::clutch_nodes(&clutch);
        let (transport, exits) = crate::mixnet::supervisor::acquire_ready_transport(
            acquirer,
            &nodes,
            std::sync::Arc::clone(publisher),
        )
        .await?;
        // Bind-time recycle: keeping only the bound lease drops the rest. A
        // report naming no drawn node (a defective host or child) refuses
        // typed, with the transport stopped and every reservation recycled.
        let Some(lease) = exit_pool::take_bound_lease(&mut clutch, &exits) else {
            transport.stop().await;
            return Err(acquire::TransportError::ExitOutsideClutch { reported: exits });
        };
        drop(clutch);
        Ok((transport, lease))
    }

    /// Births one Proven Client: a trusting birth when the bound exit's
    /// fresh proof is trusted, otherwise a proving birth whose Sentinel
    /// refusal condemns the exit and tries a successor, refusing typed when
    /// every birth failed its proof.
    pub(crate) async fn acquire_proven(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
        publisher: &crate::mixnet::driver::StatusPublisher,
    ) -> Result<(crate::mixnet::MixnetProxy, exit_pool::Reservation), acquire::TransportError> {
        for _birth in 0..MAX_PROVING_BIRTHS {
            let (transport, lease) = self.acquire_bound(acquirer, publisher).await?;
            let trusted = {
                let exits = self.exits.lock().expect("exit pool mutex");
                exits.proven(lease.node(), std::time::Instant::now())
            };
            if trusted {
                return Ok((transport, lease));
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
                    exit_pool::ExitNodeHealthVerdict::Proven,
                );
                return Ok((transport, lease));
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

    /// HYPOTHESIS: a Shared member serves repeated dials for its one
    /// holder, then retires once.
    #[tokio::test]
    async fn a_shared_member_serves_repeated_dials() {
        let member: Member<FakeTransport, Shared> = Member::new(
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
