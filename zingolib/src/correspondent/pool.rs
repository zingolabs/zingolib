//! The Correspondent Pools: ready transports Exit Rotation consumes per run.
#![forbid(unsafe_code)]

use http::Uri;

/// The Indexer Pool's ratified complement of Correspondent-Bound members.
pub(crate) const INDEXER_POOL_COMPLEMENT: usize = 2;

/// The Price Source Pool's ratified complement of one Shared-exit member.
pub(crate) const PRICE_POOL_COMPLEMENT: usize = 1;

/// The transport surface a pool member needs; production implements it on
/// the spawned `MixnetProxy`.
pub(crate) trait PoolTransport: Send + 'static {
    /// Whether the transport still reports itself ready.
    fn is_ready(&self) -> bool;
}

/// One ready member: a transport, its Exit Node, and, in the Indexer Pool
/// only, its bound Correspondent.
pub(crate) struct Member<T> {
    /// The ready transport a run consumes.
    pub(crate) transport: T,
    /// The Exit Node the transport bound.
    pub(crate) exit: String,
    /// The Correspondent assigned by draw at admission; `None` in the
    /// Shared-exit Price Source Pool.
    pub(crate) correspondent: Option<Uri>,
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
    /// The exit the previous run spent: the Price Fetch Cadence rule
    /// excludes it from the next draw.
    last_spent_exit: Option<String>,
    /// The previous consumed Transmission's Correspondent host, which the
    /// next admission draw must differ from.
    last_correspondent: Option<String>,
}

impl<T: PoolTransport> CorrespondentPool<T> {
    /// An empty pool aiming at `complement` ready members.
    pub(crate) fn new(complement: usize) -> Self {
        CorrespondentPool {
            members: Vec::new(),
            complement,
            inflight: 0,
            last_spent_exit: None,
            last_correspondent: None,
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

    /// The exits the pool currently holds, for the acquisition exclusion.
    pub(crate) fn exits(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|member| member.exit.clone())
            .collect()
    }

    /// The exits a refill's acquisition must exclude: every held exit,
    /// plus the previous run's spent exit.
    pub(crate) fn excluded_exits(&self) -> Vec<String> {
        let mut exits = self.exits();
        if let Some(spent) = &self.last_spent_exit
            && !exits.contains(spent)
        {
            exits.push(spent.clone());
        }
        exits
    }

    /// The Correspondent hosts an admission draw must differ from: every
    /// member's binding, plus the previous consumed run's.
    pub(crate) fn excluded_correspondents(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .members
            .iter()
            .filter_map(|member| member.correspondent.as_ref())
            .filter_map(|uri| uri.host().map(str::to_string))
            .collect();
        if let Some(last) = &self.last_correspondent
            && !hosts.contains(last)
        {
            hosts.push(last.clone());
        }
        hosts
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
        if let Some(taken) = &member {
            self.last_spent_exit = Some(taken.exit.clone());
            self.last_correspondent = taken
                .correspondent
                .as_ref()
                .and_then(|uri| uri.host().map(str::to_string));
        }
        Take { member, evicted }
    }

    /// Empties the pool for teardown, returning every member to stop.
    pub(crate) fn drain(&mut self) -> Vec<Member<T>> {
        std::mem::take(&mut self.members)
    }

    /// Records an exit spent outside a take — the empty-pool inline
    /// acquisition — so the next refill draw excludes it.
    pub(crate) fn note_spent_exit(&mut self, exit: String) {
        self.last_spent_exit = Some(exit);
    }
}

/// The two Correspondent Pools with the spawn context their refills need.
pub(crate) struct Pools {
    /// The Indexer Pool of Correspondent-Bound members.
    pub(crate) indexer: std::sync::Mutex<CorrespondentPool<crate::nym::MixnetProxy>>,
    /// The Price Source Pool's one Shared-exit member.
    pub(crate) price: std::sync::Mutex<CorrespondentPool<crate::nym::MixnetProxy>>,
    /// The nym-proxy binary refills spawn from; `None` until a spawned
    /// session sets it, and always `None` for attached sessions.
    binary: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl Pools {
    /// Empty pools at their ratified complements.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Pools {
            indexer: std::sync::Mutex::new(CorrespondentPool::new(INDEXER_POOL_COMPLEMENT)),
            price: std::sync::Mutex::new(CorrespondentPool::new(PRICE_POOL_COMPLEMENT)),
            binary: std::sync::Mutex::new(None),
        })
    }

    /// Records the spawned session's binary so refills can acquire.
    pub(crate) fn set_binary(&self, path: std::path::PathBuf) {
        *self.binary.lock().expect("pool binary mutex") = Some(path);
    }

    /// The refill binary, when a spawned session has set one.
    pub(crate) fn binary(&self) -> Option<std::path::PathBuf> {
        self.binary.lock().expect("pool binary mutex").clone()
    }

    /// Every exit both pools currently hold, for the session exclusion.
    pub(crate) fn exits(&self) -> Vec<String> {
        let mut exits = self.indexer.lock().expect("indexer pool mutex").exits();
        exits.extend(self.price.lock().expect("price pool mutex").exits());
        exits
    }

    /// Stops every member of both pools, for session teardown.
    pub(crate) async fn drain_all(&self) {
        let drained: Vec<Member<crate::nym::MixnetProxy>> = {
            let mut members = self.indexer.lock().expect("indexer pool mutex").drain();
            members.extend(self.price.lock().expect("price pool mutex").drain());
            members
        };
        for member in drained {
            member.transport.stop().await;
        }
    }

    /// Launches one refill task per deficit in each pool; a no-op for
    /// attached sessions, which have no binary to spawn from.
    pub(crate) fn ensure_filled(
        self: &std::sync::Arc<Self>,
        session_exits: Vec<String>,
        sync_indexer: Option<Uri>,
    ) {
        if self.binary().is_none() {
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
            let session_exits = session_exits.clone();
            let sync_indexer = sync_indexer.clone();
            tokio::spawn(async move {
                refill_one(&pools, PoolKind::Indexer, session_exits, sync_indexer).await;
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
            let session_exits = session_exits.clone();
            tokio::spawn(async move {
                refill_one(&pools, PoolKind::Price, session_exits, None).await;
            });
        }
    }
}

/// Which Correspondent Pool a refill serves.
#[derive(Clone, Copy)]
pub(crate) enum PoolKind {
    /// The Correspondent-Bound Indexer Pool.
    Indexer,
    /// The Shared-exit Price Source Pool.
    Price,
}

/// One refill: acquire a ready transport excluding every in-use and spent
/// exit, draw the Correspondent for the Indexer kind, and admit.
async fn refill_one(
    pools: &std::sync::Arc<Pools>,
    kind: PoolKind,
    session_exits: Vec<String>,
    sync_indexer: Option<Uri>,
) {
    let pool = match kind {
        PoolKind::Indexer => &pools.indexer,
        PoolKind::Price => &pools.price,
    };
    let Some(binary) = pools.binary() else {
        pool.lock().expect("pool mutex").note_refill_finished();
        return;
    };
    let excluded = {
        let mut exits = session_exits;
        for exit in pool.lock().expect("pool mutex").excluded_exits() {
            if !exits.contains(&exit) {
                exits.push(exit);
            }
        }
        exits
    };
    match crate::nym::supervisor::spawn_ready_pool_transport(&binary, &excluded).await {
        Ok((transport, exit)) => {
            let correspondent = match kind {
                PoolKind::Price => None,
                PoolKind::Indexer => {
                    let excluded_hosts = pool.lock().expect("pool mutex").excluded_correspondents();
                    match draw_correspondent(sync_indexer.as_ref(), &excluded_hosts) {
                        Some(drawn) => Some(drawn),
                        None => {
                            pool.lock().expect("pool mutex").note_refill_finished();
                            log::warn!(
                                "indexer pool refill: no eligible Correspondent to bind; \
                                 the acquired transport is released"
                            );
                            transport.stop().await;
                            return;
                        }
                    }
                }
            };
            let mut pool = pool.lock().expect("pool mutex");
            pool.admit(Member {
                transport,
                exit,
                correspondent,
            });
            pool.note_refill_finished();
        }
        Err(cause) => {
            pool.lock().expect("pool mutex").note_refill_finished();
            log::warn!("pool refill failed: {cause}");
        }
    }
}

/// Correspondent Selection at admission: a uniform draw over the eligible
/// Correspondents, excluding the pool's bound and previously consumed hosts.
fn draw_correspondent(sync_indexer: Option<&Uri>, excluded_hosts: &[String]) -> Option<Uri> {
    use rand::seq::SliceRandom as _;
    let eligible = crate::correspondent::eligible_correspondents(sync_indexer).ok()?;
    let drawable: Vec<Uri> = eligible
        .into_iter()
        .filter(|uri| {
            uri.host()
                .is_none_or(|host| !excluded_hosts.iter().any(|excluded| excluded == host))
        })
        .collect();
    drawable.choose(&mut rand::rngs::OsRng).cloned()
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

    fn member(exit: &str, correspondent: Option<&str>, ready: bool) -> Member<FakeTransport> {
        Member {
            transport: FakeTransport { ready },
            exit: exit.to_string(),
            correspondent: correspondent.map(|c| c.parse().expect("test uri parses")),
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
        pool.admit(member("exit-a", Some("https://zec.rocks:443"), true));
        assert_eq!(pool.deficit(), 0, "one member and one in flight");
    }

    /// HYPOTHESIS: a take skips and evicts dead members, consumes the
    /// oldest ready one, and records its exit and Correspondent as the
    /// next draw's exclusions.
    #[test]
    fn a_take_evicts_the_dead_and_records_the_spent() {
        let mut pool: CorrespondentPool<FakeTransport> =
            CorrespondentPool::new(INDEXER_POOL_COMPLEMENT);
        pool.admit(member("exit-dead", Some("https://l.ombie.cash:443"), false));
        pool.admit(member("exit-live", Some("https://zec.rocks:443"), true));
        let take = pool.take();
        assert_eq!(take.evicted.len(), 1, "the dead member is evicted");
        let taken = take.member.expect("the live member is taken");
        assert_eq!(taken.exit, "exit-live");
        assert_eq!(pool.excluded_exits(), vec!["exit-live".to_string()]);
        assert_eq!(
            pool.excluded_correspondents(),
            vec!["zec.rocks".to_string()],
            "the next admission draw must differ from the spent Correspondent"
        );
    }

    /// HYPOTHESIS: an empty pool takes nothing and evicts nothing — the
    /// caller then waits on its own acquisition, never reusing a spent
    /// tunnel.
    #[test]
    fn an_empty_pool_is_a_miss_not_a_reuse() {
        let mut pool: CorrespondentPool<FakeTransport> = CorrespondentPool::new(1);
        pool.admit(member("exit-a", None, true));
        let first = pool.take();
        assert!(first.member.is_some());
        let second = pool.take();
        assert!(second.member.is_none());
        assert!(second.evicted.is_empty());
        assert_eq!(
            pool.excluded_exits(),
            vec!["exit-a".to_string()],
            "the spent exit stays excluded for the refill"
        );
    }

    /// HYPOTHESIS: the Shared price member binds no Correspondent, and its
    /// exclusions are exit-only.
    #[test]
    fn a_shared_member_excludes_exits_only() {
        let mut pool: CorrespondentPool<FakeTransport> =
            CorrespondentPool::new(PRICE_POOL_COMPLEMENT);
        pool.admit(member("exit-a", None, true));
        let take = pool.take();
        assert!(take.member.expect("taken").correspondent.is_none());
        assert!(pool.excluded_correspondents().is_empty());
        assert_eq!(pool.excluded_exits(), vec!["exit-a".to_string()]);
    }
}
