//! The Correspondent Pools: ready transports Exit Rotation consumes per run.
#![forbid(unsafe_code)]

pub(crate) mod exit_pool;

use crate::mixnet::acquire;

/// The Indexer Pool's ratified complement of Exit-Bound members.
pub(crate) const INDEXER_POOL_COMPLEMENT: usize = 2;

/// The Price Source Pool's ratified complement of one Shared-exit member.
pub(crate) const PRICE_POOL_COMPLEMENT: usize = 1;

/// The transport surface a pool member needs; production implements it on
/// the spawned `MixnetProxy`.
pub(crate) trait PoolTransport: Send + 'static {
    /// Whether the transport still reports itself ready.
    fn is_ready(&self) -> bool;
    /// The transport's local SOCKS5 address, while it lives.
    fn socks5_addr(&self) -> Option<std::net::SocketAddr>;
    /// Tears the transport down.
    fn stop(self) -> impl std::future::Future<Output = ()> + Send;
}

/// The sealed category of a bound exit's Correspondent exposure.
pub(crate) trait ExitUse: sealed::Sealed + Send + 'static {}

/// The category serving exactly one Correspondent, whose dial consumes the
/// member.
pub(crate) enum Exclusive {}

/// The category fanning out to many Correspondents for its one holder.
pub(crate) enum Shared {}

mod sealed {
    pub(crate) trait Sealed {}
    impl Sealed for super::Exclusive {}
    impl Sealed for super::Shared {}
}

impl ExitUse for Exclusive {}
impl ExitUse for Shared {}

/// One ready member: an Exit-Bound transport and the owning lease of the
/// Exit Node it bound, in the exit-use category `U`.
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

    /// Whether the member's transport still reports itself ready.
    pub(crate) fn is_ready(&self) -> bool {
        self.transport.is_ready()
    }

    /// The bound Exit Node's identity.
    #[cfg(test)]
    pub(crate) fn node(&self) -> &crate::mixnet::ExitNodeId {
        self.lease.node()
    }

    /// Stops the transport and recycles the lease.
    pub(crate) async fn retire(self) {
        self.transport.stop().await;
    }
}

impl<T: PoolTransport> Member<T, Exclusive> {
    /// The one dial: consumes the member, yielding the tunnel address for a
    /// single Correspondent contact and the spent holder to retire.
    pub(crate) fn dial(self) -> (Option<std::net::SocketAddr>, SpentExit<T>) {
        let addr = self.transport.socks5_addr();
        (
            addr,
            SpentExit {
                transport: self.transport,
                lease: self.lease,
            },
        )
    }
}

impl<T: PoolTransport> Member<T, Shared> {
    /// The tunnel address for one more Correspondent contact, while the
    /// transport lives.
    pub(crate) fn addr(&self) -> Option<std::net::SocketAddr> {
        self.transport.socks5_addr()
    }
}

/// A dialled Exclusive member, capable only of teardown.
pub(crate) struct SpentExit<T> {
    transport: T,
    lease: exit_pool::Reservation,
}

impl<T: PoolTransport> SpentExit<T> {
    /// The spent exit's identity, for the attempt record.
    pub(crate) fn node(&self) -> &crate::mixnet::ExitNodeId {
        self.lease.node()
    }

    /// Stops the transport and recycles the lease.
    pub(crate) async fn retire(self) {
        self.transport.stop().await;
    }
}

/// What a take found: the member to consume, and any dead members evicted
/// on the way, which the caller tears down.
pub(crate) struct Take<T, U: ExitUse> {
    /// The ready member, when one exists.
    pub(crate) member: Option<Member<T, U>>,
    /// Members found dead during the scan, to be retired by the caller.
    pub(crate) evicted: Vec<Member<T, U>>,
}

/// One Correspondent Pool's synchronous state; async refill orchestration
/// lives with the caller that owns the transport factory.
pub(crate) struct CorrespondentPool<T, U: ExitUse> {
    members: Vec<Member<T, U>>,
    complement: usize,
    /// Refills already launched but not yet admitted, so deficit never
    /// over-spawns.
    inflight: usize,
}

impl<T: PoolTransport, U: ExitUse> CorrespondentPool<T, U> {
    /// An empty pool aiming at `complement` ready members.
    pub(crate) fn new(complement: usize) -> Self {
        CorrespondentPool {
            members: Vec::new(),
            complement,
            inflight: 0,
        }
    }

    /// How many refills the pool wants launched right now.
    pub(crate) fn deficit(&self) -> usize {
        self.complement
            .saturating_sub(self.members.len() + self.inflight)
    }

    /// Marks one refill launched, so a second scan does not over-spawn.
    pub(crate) fn note_refill_launched(&mut self) {
        self.inflight += 1;
    }

    /// Marks one launched refill finished, admitted or failed.
    pub(crate) fn note_refill_finished(&mut self) {
        self.inflight = self.inflight.saturating_sub(1);
    }

    /// Admits a ready member.
    pub(crate) fn admit(&mut self, member: Member<T, U>) {
        self.members.push(member);
    }

    /// Takes the oldest still-ready member, evicting dead ones on the way.
    pub(crate) fn take(&mut self) -> Take<T, U> {
        let mut evicted = Vec::new();
        let mut member = None;
        while member.is_none() && !self.members.is_empty() {
            let candidate = self.members.remove(0);
            if candidate.is_ready() {
                member = Some(candidate);
            } else {
                evicted.push(candidate);
            }
        }
        Take { member, evicted }
    }

    /// Empties the pool for teardown, returning every member to retire.
    pub(crate) fn drain(&mut self) -> Vec<Member<T, U>> {
        std::mem::take(&mut self.members)
    }
}

/// The two Correspondent Pools with the spawn context their refills need.
pub(crate) struct Pools {
    /// The Indexer Pool of Exclusive-exit members a Transmission's pulls
    /// consume.
    pub(crate) indexer: std::sync::Mutex<CorrespondentPool<crate::mixnet::MixnetProxy, Exclusive>>,
    /// The Price Source Pool's one Shared-exit member.
    pub(crate) price: std::sync::Mutex<CorrespondentPool<crate::mixnet::MixnetProxy, Shared>>,
    /// The session's sole issuer of Exit Node Reservations; reservations
    /// hold a weak ledger handle and recycle themselves on drop.
    pub(crate) exits: std::sync::Arc<std::sync::Mutex<exit_pool::ExitPool>>,
    /// What refills acquire transports from; `None` until a session sets
    /// one, and always `None` for attached sessions.
    acquirer:
        std::sync::Mutex<Option<std::sync::Arc<dyn crate::mixnet::acquire::TransportAcquirable>>>,
    /// Bumped by every drain, so a refill launched before the drain refuses
    /// to admit its child into the drained pool.
    generation: std::sync::atomic::AtomicU64,
}

impl Pools {
    /// Empty pools at their ratified complements.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Pools {
            indexer: std::sync::Mutex::new(CorrespondentPool::new(INDEXER_POOL_COMPLEMENT)),
            price: std::sync::Mutex::new(CorrespondentPool::new(PRICE_POOL_COMPLEMENT)),
            exits: std::sync::Arc::new(std::sync::Mutex::new(exit_pool::ExitPool::default())),
            acquirer: std::sync::Mutex::new(None),
            generation: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// The current drain generation, captured by a refill at launch.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Draws a Clutch, seeding the Exit Pool from the directory first when
    /// this session has not yet learned the population.
    pub(crate) async fn draw_clutch(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    ) -> Result<Vec<exit_pool::Reservation>, acquire::TransportError> {
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

    /// This session's acquirer, when one is set.
    pub(crate) fn acquirer(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::mixnet::acquire::TransportAcquirable>> {
        self.acquirer.lock().expect("pool acquirer mutex").clone()
    }

    /// Takes `pool`'s next live member, or acquires a fresh one in the
    /// category the call site declares, retiring the dead on the way.
    pub(crate) async fn take_or_acquire<U: ExitUse>(
        &self,
        pool: for<'a> fn(
            &'a Pools,
        )
            -> &'a std::sync::Mutex<CorrespondentPool<crate::mixnet::MixnetProxy, U>>,
    ) -> Result<Member<crate::mixnet::MixnetProxy, U>, acquire::TransportError> {
        let take = {
            let mut pool = pool(self).lock().expect("pool mutex");
            pool.take()
        };
        for dead in take.evicted {
            dead.retire().await;
        }
        if let Some(member) = take.member {
            return Ok(member);
        }
        let acquirer = self.acquirer().ok_or(acquire::TransportError::NoAcquirer)?;
        let (transport, lease) = self.acquire_bound(acquirer.as_ref()).await?;
        Ok(Member::new(transport, lease))
    }

    /// Acquires one ready transport over a fresh Clutch, keeping only the
    /// bound exit's lease.
    async fn acquire_bound(
        &self,
        acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    ) -> Result<(crate::mixnet::MixnetProxy, exit_pool::Reservation), acquire::TransportError> {
        let mut clutch = self.draw_clutch(acquirer).await?;
        let nodes = exit_pool::clutch_nodes(&clutch);
        let (transport, exit) =
            crate::mixnet::supervisor::acquire_ready_transport(acquirer, &nodes).await?;
        // Bind-time recycle: keeping only the bound lease drops the rest.
        let bound = clutch
            .iter()
            .position(|reservation| reservation.node() == &exit)
            .expect("the bound exit is one of the clutch's nodes");
        let lease = clutch.swap_remove(bound);
        drop(clutch);
        Ok((transport, lease))
    }

    /// Stops every member of both pools and forbids the session's in-flight
    /// refills from admitting, for session teardown.
    pub(crate) async fn drain_all(&self) {
        // Bump first, so a refill mid-acquisition sees the new generation and
        // refuses to admit; then clear the acquirer so no later ensure_filled
        // relaunches against a torn-down session.
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        *self.acquirer.lock().expect("pool acquirer mutex") = None;
        let indexer_drained = self.indexer.lock().expect("indexer pool mutex").drain();
        let price_drained = self.price.lock().expect("price pool mutex").drain();
        for member in indexer_drained {
            member.retire().await;
        }
        for member in price_drained {
            member.retire().await;
        }
    }

    /// Launches one refill task per deficit in each pool; a no-op for
    /// attached sessions, which have no binary to spawn from.
    pub(crate) fn ensure_filled(self: &std::sync::Arc<Self>) {
        if self.acquirer().is_none() {
            return;
        }
        let indexer_deficit = {
            let mut pool = self.indexer.lock().expect("indexer pool mutex");
            let deficit = pool.deficit();
            for _ in 0..deficit {
                pool.note_refill_launched();
            }
            deficit
        };
        for _ in 0..indexer_deficit {
            let pools = std::sync::Arc::clone(self);
            tokio::spawn(async move {
                refill_one(&pools, |pools: &Pools| &pools.indexer).await;
            });
        }
        let price_deficit = {
            let mut pool = self.price.lock().expect("price pool mutex");
            let deficit = pool.deficit();
            for _ in 0..deficit {
                pool.note_refill_launched();
            }
            deficit
        };
        for _ in 0..price_deficit {
            let pools = std::sync::Arc::clone(self);
            tokio::spawn(async move {
                refill_one(&pools, |pools: &Pools| &pools.price).await;
            });
        }
    }
}

/// One refill of `pool`: acquire a ready transport over a fresh Clutch,
/// then admit it in the pool's own exit-use category.
async fn refill_one<U: ExitUse>(
    pools: &std::sync::Arc<Pools>,
    pool: for<'a> fn(
        &'a Pools,
    ) -> &'a std::sync::Mutex<CorrespondentPool<crate::mixnet::MixnetProxy, U>>,
) {
    // The generation this refill was launched under; a drain that lands
    // before we admit bumps it, and we then refuse rather than admit a live
    // child into the drained pool.
    let launched_at = pools.generation();
    let Some(acquirer) = pools.acquirer() else {
        pool(pools)
            .lock()
            .expect("pool mutex")
            .note_refill_finished();
        return;
    };
    match pools.acquire_bound(acquirer.as_ref()).await {
        Ok((transport, lease)) => {
            // The generation check and the admit share one lock hold, so a
            // drain cannot slip between them and orphan the child.
            let stale: Option<Member<crate::mixnet::MixnetProxy, U>> = {
                let mut pool = pool(pools).lock().expect("pool mutex");
                pool.note_refill_finished();
                if pools.generation() == launched_at {
                    pool.admit(Member::new(transport, lease));
                    None
                } else {
                    Some(Member::new(transport, lease))
                }
            };
            if let Some(member) = stale {
                // A drain landed while we acquired: retire rather than admit
                // into a torn-down session.
                member.retire().await;
            }
        }
        Err(cause) => {
            pool(pools)
                .lock()
                .expect("pool mutex")
                .note_refill_finished();
            log::warn!("pool refill failed: {cause}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransport {
        ready: bool,
    }

    impl PoolTransport for FakeTransport {
        fn is_ready(&self) -> bool {
            self.ready
        }

        fn socks5_addr(&self) -> Option<std::net::SocketAddr> {
            self.ready
                .then(|| "127.0.0.1:7".parse().expect("the fake address parses"))
        }

        fn stop(self) -> impl std::future::Future<Output = ()> + Send {
            std::future::ready(())
        }
    }

    fn member(exit: &str, ready: bool) -> Member<FakeTransport, Exclusive> {
        Member::new(
            FakeTransport { ready },
            exit_pool::Reservation::dangling_for_test(exit),
        )
    }

    /// HYPOTHESIS: the deficit counts in-flight refills, so two scans never
    /// launch more acquisitions than the complement.
    #[test]
    fn the_deficit_never_overspawns() {
        let mut pool: CorrespondentPool<FakeTransport, Exclusive> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        assert_eq!(pool.deficit(), 2);
        pool.note_refill_launched();
        assert_eq!(pool.deficit(), 1);
        pool.note_refill_launched();
        assert_eq!(pool.deficit(), 0);
        pool.note_refill_finished();
        pool.admit(member("exit-a", true));
        assert_eq!(pool.deficit(), 0, "one member and one in flight");
    }

    /// HYPOTHESIS: a take skips and evicts dead members, consumes the
    /// oldest ready one, and records its exit as the next draw's
    /// exclusion.
    #[test]
    fn a_take_evicts_the_dead_and_records_the_spent() {
        let mut pool: CorrespondentPool<FakeTransport, Exclusive> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        pool.admit(member("exit-dead", false));
        pool.admit(member("exit-live", true));
        let take = pool.take();
        assert_eq!(take.evicted.len(), 1, "the dead member is evicted");
        let taken = take.member.expect("the live member is taken");
        assert_eq!(taken.node(), &crate::mixnet::ExitNodeId::from("exit-live"));
        assert!(pool.take().member.is_none(), "both members left the pool");
    }

    /// HYPOTHESIS: an empty pool takes nothing and evicts nothing — the
    /// caller then waits on its own acquisition, never reusing a spent
    /// tunnel.
    #[test]
    fn an_empty_pool_is_a_miss_not_a_reuse() {
        let mut pool: CorrespondentPool<FakeTransport, Exclusive> = CorrespondentPool::new(1);
        pool.admit(member("exit-a", true));
        let first = pool.take();
        assert!(first.member.is_some());
        let second = pool.take();
        assert!(second.member.is_none());
        assert!(second.evicted.is_empty());
    }

    /// HYPOTHESIS: consecutive takes never repeat an exit, because a member
    /// is consumed rather than lent.
    #[test]
    fn consecutive_takes_never_repeat_an_exit() {
        let mut pool: CorrespondentPool<FakeTransport, Exclusive> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        pool.admit(member("exit-a", true));
        pool.admit(member("exit-b", true));
        let first = pool.take().member.expect("first take");
        let second = pool.take().member.expect("second take");
        assert_ne!(
            first.node(),
            second.node(),
            "a member is consumed, never reused"
        );
        assert!(pool.take().member.is_none(), "the pool is spent");
    }

    /// HYPOTHESIS: draining bumps the generation and clears the acquirer, the
    /// two effects a refill launched before the drain checks to refuse
    /// admitting its child into the torn-down session.
    #[tokio::test]
    async fn draining_bumps_the_generation_and_clears_the_acquirer() {
        let pools = Pools::new();
        let before = pools.generation();
        // The pools hold no members, so the drain has nothing to stop.
        pools.drain_all().await;
        assert_ne!(
            pools.generation(),
            before,
            "a drain must bump the generation so in-flight refills refuse"
        );
        assert!(
            pools.acquirer().is_none(),
            "a drain must clear the acquirer so ensure_filled cannot relaunch"
        );
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

    /// HYPOTHESIS: an Exclusive dial consumes the member, leaving only a
    /// spent holder that still names its exit for the attempt record.
    #[tokio::test]
    async fn an_exclusive_dial_is_terminal() {
        let member = member("exit-exclusive", true);
        let (addr, spent) = member.dial();
        assert!(addr.is_some(), "a live exclusive member dials once");
        assert_eq!(
            spent.node(),
            &crate::mixnet::ExitNodeId::from("exit-exclusive")
        );
        spent.retire().await;
    }
}
