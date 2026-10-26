//! The three proven exits a boot establishes, and the roles they carry.
#![forbid(unsafe_code)]

use futures::StreamExt as _;

use crate::mixnet::acquire::{TransportAcquirable, TransportError};
use crate::mixnet::pools::{Pools, ProvenBirth};

/// How many exits a boot proves before it opens a prompt: one for each
/// role, plus the spare that lets a failing role cost no birth.
pub(crate) const ROLE_EXIT_COUNT: usize = 3;

/// The job one Exit Node holds for a Nym epoch, assigned in the order the
/// exits prove themselves (ADR 0045, narrowed to three by ADR 0063).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    /// Fetches the price.
    PriceFetch,
    /// Carries Transmissions, and the only role whose client persists.
    IndexerClient,
    /// Held proven and unassigned, taken up when a role's exit fails.
    Spare,
}

/// The three proven births a boot establishes, each already bound to the
/// exit that carries its role.
pub(crate) struct RoleExits {
    /// The first exit to prove itself, whose client fetches the price.
    pub(crate) price: ProvenBirth,
    /// The second, whose client persists to carry Transmissions.
    pub(crate) indexer: ProvenBirth,
    /// The third, held proven against a role's exit failing.
    pub(crate) spare: ProvenBirth,
}

/// Proves [`ROLE_EXIT_COUNT`] exits by racing that many acquisitions,
/// assigning each a [`Role`] in the order it confirms, and recording the
/// role-to-exit binding that outlives every client.
///
/// The acquisitions race, so the wall clock is the slowest of the three to
/// confirm rather than their sum, and each already retries within its own
/// birth budget. An acquisition that exhausts that budget refuses the whole
/// boot: an online session that cannot prove three exits cannot fill its
/// roles.
pub(crate) async fn prove_role_exits<A>(
    pools: &Pools,
    acquirer: &A,
) -> Result<RoleExits, TransportError>
where
    A: TransportAcquirable + ?Sized,
{
    let mut acquisitions = (0..ROLE_EXIT_COUNT)
        .map(|_| pools.acquire_proven(acquirer))
        .collect::<futures::stream::FuturesUnordered<_>>();

    let mut proven: Vec<ProvenBirth> = Vec::with_capacity(ROLE_EXIT_COUNT);
    while let Some(outcome) = acquisitions.next().await {
        match outcome {
            Ok(birth) => proven.push(birth),
            Err(refusal) => {
                // One acquisition exhausting its births means the mixnet
                // refused this session everything it asked for. Retire what
                // already proved rather than leaving clients behind a refusal.
                drop(acquisitions);
                for birth in proven {
                    birth.transport.stop().await;
                }
                return Err(refusal);
            }
        }
    }

    let mut assigned = proven.into_iter();
    let mut take = |role: Role| {
        let birth = assigned
            .next()
            .expect("the race yielded ROLE_EXIT_COUNT births, one per role taken here");
        pools.assign_role(role, birth.lease.node().clone());
        birth
    };
    Ok(RoleExits {
        price: take(Role::PriceFetch),
        indexer: take(Role::IndexerClient),
        spare: take(Role::Spare),
    })
}
