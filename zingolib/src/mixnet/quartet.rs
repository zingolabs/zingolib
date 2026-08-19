//! The four proven exits a boot establishes, and the roles they carry.
#![forbid(unsafe_code)]

use futures::StreamExt as _;

use crate::correspondent::pool::{Pools, ProvenBirth};
use crate::mixnet::acquire::{TransportAcquirable, TransportError};

/// How many exits a boot proves before it opens a prompt: one for each
/// role, plus the spare that lets a failing role cost no birth.
pub(crate) const QUARTET_SIZE: usize = 4;

/// The job one Exit Node holds for a Nym epoch, assigned in the order the
/// exits prove themselves (ADR 0045).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Role {
    /// Runs the Server-Selection Sweep.
    IndexerSweep,
    /// Fetches the price.
    PriceFetch,
    /// Carries Transmissions, and the only role whose client persists.
    IndexerClient,
    /// Held proven and unassigned, taken up when a role's exit fails.
    Spare,
}

/// The four proven births a boot establishes, each already bound to the
/// exit that carries its role.
pub(crate) struct Quartet {
    /// The first exit to prove itself, whose client runs the sweep.
    pub(crate) sweep: ProvenBirth,
    /// The second, whose client fetches the price.
    pub(crate) price: ProvenBirth,
    /// The third, whose client persists to carry Transmissions.
    pub(crate) indexer: ProvenBirth,
    /// The fourth, held proven against a role's exit failing.
    pub(crate) spare: ProvenBirth,
}

/// Proves [`QUARTET_SIZE`] exits by racing that many births, assigning each
/// a [`Role`] in the order it confirms, and recording the role-to-exit
/// binding that outlives every client.
///
/// The lanes race, so the wall clock is the slowest of the four to confirm
/// rather than their sum, and each lane already retries within its own
/// birth budget. A lane that exhausts that budget refuses the whole boot:
/// an online session that cannot prove four exits cannot fill its roles.
pub(crate) async fn prove_quartet<A>(pools: &Pools, acquirer: &A) -> Result<Quartet, TransportError>
where
    A: TransportAcquirable + ?Sized,
{
    let mut lanes = (0..QUARTET_SIZE)
        .map(|_| pools.acquire_proven(acquirer))
        .collect::<futures::stream::FuturesUnordered<_>>();

    let mut proven: Vec<ProvenBirth> = Vec::with_capacity(QUARTET_SIZE);
    while let Some(outcome) = lanes.next().await {
        match outcome {
            Ok(birth) => proven.push(birth),
            Err(refusal) => {
                // One lane exhausting its births means the mixnet refused
                // this session everything it asked for. Retire what already
                // proved rather than leaving clients behind a refusal.
                drop(lanes);
                for birth in proven {
                    birth.transport.stop().await;
                }
                return Err(refusal);
            }
        }
    }

    let mut assigned = proven.into_iter();
    let mut take = |role: Role| {
        let birth = assigned.next().expect("every lane yielded a birth");
        pools.assign_role(role, birth.lease.node().clone());
        birth
    };
    Ok(Quartet {
        sweep: take(Role::IndexerSweep),
        price: take(Role::PriceFetch),
        indexer: take(Role::IndexerClient),
        spare: take(Role::Spare),
    })
}
