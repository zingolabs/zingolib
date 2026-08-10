//! The Correspondent Pools: ready transports Exit Rotation consumes per run.
#![forbid(unsafe_code)]

pub(crate) mod exit_pool;

/// The Indexer Pool's ratified complement of Exit-Bound members.
pub(crate) const INDEXER_POOL_COMPLEMENT: usize = 2;

/// The Price Source Pool's ratified complement of one Shared-exit member.
pub(crate) const PRICE_POOL_COMPLEMENT: usize = 1;

/// The transport surface a pool member needs; production implements it on
/// the spawned `MixnetProxy`.
pub(crate) trait PoolTransport: Send + 'static {
    /// Whether the transport still reports itself ready.
    fn is_ready(&self) -> bool;
}

/// One ready member: an Exit-Bound transport and the Exit Node it bound.
pub(crate) struct Member<T> {
    /// The ready transport a run consumes.
    pub(crate) transport: T,
    /// The Exit Node the transport bound.
    pub(crate) exit: String,
}

/// What a take found: the member to consume, and any dead members evicted
/// on the way, which the caller tears down.
pub(crate) struct Take<T> {
    /// The ready member, when one exists.
    pub(crate) member: Option<Member<T>>,
    /// Members found dead during the scan, to be stopped by the caller.
    pub(crate) evicted: Vec<Member<T>>,
}

/// One Correspondent Pool's synchronous state; async refill orchestration
/// lives with the caller that owns the transport factory.
pub(crate) struct CorrespondentPool<T> {
    members: Vec<Member<T>>,
    complement: usize,
    /// Refills already launched but not yet admitted, so deficit never
    /// over-spawns.
    inflight: usize,
}

impl<T: PoolTransport> CorrespondentPool<T> {
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
    pub(crate) fn admit(&mut self, member: Member<T>) {
        self.members.push(member);
    }

    /// Takes the oldest still-ready member, evicting dead ones on the way.
    pub(crate) fn take(&mut self) -> Take<T> {
        let mut evicted = Vec::new();
        let mut member = None;
        while member.is_none() && !self.members.is_empty() {
            let candidate = self.members.remove(0);
            if candidate.transport.is_ready() {
                member = Some(candidate);
            } else {
                evicted.push(candidate);
            }
        }
        Take { member, evicted }
    }

    /// Empties the pool for teardown, returning every member to stop.
    pub(crate) fn drain(&mut self) -> Vec<Member<T>> {
        std::mem::take(&mut self.members)
    }
}

/// The two Correspondent Pools with the spawn context their refills need.
pub(crate) struct Pools {
    /// The Indexer Pool of Exit-Bound members.
    pub(crate) indexer: std::sync::Mutex<CorrespondentPool<crate::nym::MixnetProxy>>,
    /// The Price Source Pool's one Shared-exit member.
    pub(crate) price: std::sync::Mutex<CorrespondentPool<crate::nym::MixnetProxy>>,
    /// The session's sole issuer of Exit Node Reservations.
    pub(crate) exits: std::sync::Mutex<exit_pool::ExitPool>,
    /// What refills acquire transports from; `None` until a session sets
    /// one, and always `None` for attached sessions.
    acquirer:
        std::sync::Mutex<Option<std::sync::Arc<dyn crate::nym::acquire::TransportAcquirable>>>,
}

impl Pools {
    /// Empty pools at their ratified complements.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Pools {
            indexer: std::sync::Mutex::new(CorrespondentPool::new(INDEXER_POOL_COMPLEMENT)),
            price: std::sync::Mutex::new(CorrespondentPool::new(PRICE_POOL_COMPLEMENT)),
            exits: std::sync::Mutex::new(exit_pool::ExitPool::default()),
            acquirer: std::sync::Mutex::new(None),
        })
    }

    /// Draws a Clutch, seeding the Exit Pool from the directory first when
    /// this session has not yet learned the population.
    pub(crate) async fn draw_clutch(
        &self,
        acquirer: &dyn crate::nym::acquire::TransportAcquirable,
    ) -> Result<Vec<String>, crate::nym::acquire::AcquireError> {
        let seeded = self.exits.lock().expect("exit pool mutex").is_seeded();
        if !seeded {
            let discovered = acquirer.discover().await?;
            self.exits.lock().expect("exit pool mutex").seed(discovered);
        }
        Ok(self.exits.lock().expect("exit pool mutex").draw_clutch()?)
    }

    /// Returns one transport's Exclusive Lease to the Exit Pool when its
    /// lifecycle ends.
    pub(crate) fn recycle_lease(&self, exit: String) {
        self.exits
            .lock()
            .expect("exit pool mutex")
            .recycle(std::iter::once(exit));
    }

    /// Records what this session acquires transports from.
    pub(crate) fn set_acquirer(
        &self,
        acquirer: std::sync::Arc<dyn crate::nym::acquire::TransportAcquirable>,
    ) {
        *self.acquirer.lock().expect("pool acquirer mutex") = Some(acquirer);
    }

    /// This session's acquirer, when one is set.
    pub(crate) fn acquirer(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::nym::acquire::TransportAcquirable>> {
        self.acquirer.lock().expect("pool acquirer mutex").clone()
    }

    /// Stops every member of both pools, for session teardown.
    pub(crate) async fn drain_all(&self) {
        let drained: Vec<Member<crate::nym::MixnetProxy>> = {
            let mut members = self.indexer.lock().expect("indexer pool mutex").drain();
            members.extend(self.price.lock().expect("price pool mutex").drain());
            members
        };
        for member in drained {
            let exit = member.exit.clone();
            member.transport.stop().await;
            self.recycle_lease(exit);
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
                refill_one(&pools, PoolKind::Indexer).await;
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
                refill_one(&pools, PoolKind::Price).await;
            });
        }
    }
}

/// Which Correspondent Pool a refill serves.
#[derive(Clone, Copy)]
pub(crate) enum PoolKind {
    /// The Indexer Pool a Transmission's pulls consume.
    Indexer,
    /// The Shared-exit Price Source Pool.
    Price,
}

/// One refill: acquire a ready transport excluding every in-use and spent
/// exit, then admit it.
async fn refill_one(pools: &std::sync::Arc<Pools>, kind: PoolKind) {
    let pool = match kind {
        PoolKind::Indexer => &pools.indexer,
        PoolKind::Price => &pools.price,
    };
    let Some(acquirer) = pools.acquirer() else {
        pool.lock().expect("pool mutex").note_refill_finished();
        return;
    };
    let clutch = match pools.draw_clutch(acquirer.as_ref()).await {
        Ok(clutch) => clutch,
        Err(cause) => {
            pool.lock().expect("pool mutex").note_refill_finished();
            log::warn!("pool refill drew no clutch: {cause}");
            return;
        }
    };
    match crate::nym::supervisor::acquire_ready_transport(acquirer.as_ref(), &clutch).await {
        Ok((transport, exit)) => {
            // Bind-time recycle: every reservation but the bound one goes
            // back at once, so the pool drains only by what is leased.
            pools
                .exits
                .lock()
                .expect("exit pool mutex")
                .recycle(clutch.into_iter().filter(|node| node != &exit));
            let mut pool = pool.lock().expect("pool mutex");
            pool.admit(Member { transport, exit });
            pool.note_refill_finished();
        }
        Err(cause) => {
            pools.exits.lock().expect("exit pool mutex").recycle(clutch);
            pool.lock().expect("pool mutex").note_refill_finished();
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
    }

    fn member(exit: &str, ready: bool) -> Member<FakeTransport> {
        Member {
            transport: FakeTransport { ready },
            exit: exit.to_string(),
        }
    }

    /// HYPOTHESIS: the deficit counts in-flight refills, so two scans never
    /// launch more acquisitions than the complement.
    #[test]
    fn the_deficit_never_overspawns() {
        let mut pool: CorrespondentPool<FakeTransport> =
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
        let mut pool: CorrespondentPool<FakeTransport> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        pool.admit(member("exit-dead", false));
        pool.admit(member("exit-live", true));
        let take = pool.take();
        assert_eq!(take.evicted.len(), 1, "the dead member is evicted");
        let taken = take.member.expect("the live member is taken");
        assert_eq!(taken.exit, "exit-live");
        assert!(pool.take().member.is_none(), "both members left the pool");
    }

    /// HYPOTHESIS: an empty pool takes nothing and evicts nothing — the
    /// caller then waits on its own acquisition, never reusing a spent
    /// tunnel.
    #[test]
    fn an_empty_pool_is_a_miss_not_a_reuse() {
        let mut pool: CorrespondentPool<FakeTransport> = CorrespondentPool::new(1);
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
        let mut pool: CorrespondentPool<FakeTransport> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        pool.admit(member("exit-a", true));
        pool.admit(member("exit-b", true));
        let first = pool.take().member.expect("first take").exit;
        let second = pool.take().member.expect("second take").exit;
        assert_ne!(first, second, "a member is consumed, never reused");
        assert!(pool.take().member.is_none(), "the pool is spent");
    }
}
