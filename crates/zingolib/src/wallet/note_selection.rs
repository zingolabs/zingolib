use std::collections::BTreeMap;

use pepper_sync::wallet::{IronwoodNote, OrchardNote, OutputId, OutputInterface, SaplingNote};
use zcash_client_backend::data_api::{MaxSpendMode, TargetValue};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::ShieldedPool;

pub(crate) struct Candidate<N> {
    pub(crate) note: N,
    pub(crate) certain: bool,
    pub(crate) reserved: bool,
}

#[derive(Default)]
pub(crate) struct Candidates {
    pub(crate) sapling: Vec<Candidate<SaplingNote>>,
    pub(crate) orchard: Vec<Candidate<OrchardNote>>,
    pub(crate) ironwood: Vec<Candidate<IronwoodNote>>,
}

#[derive(Default)]
pub(crate) struct PoolNotes {
    pub(crate) sapling: Vec<SaplingNote>,
    pub(crate) orchard: Vec<OrchardNote>,
    pub(crate) ironwood: Vec<IronwoodNote>,
}

pub(crate) enum Selection {
    Sufficient { notes: PoolNotes },
    Insufficient { notes: PoolNotes },
    AllSpendable { notes: PoolNotes },
    EverythingUnsupported,
}

#[derive(Clone, Copy)]
enum Budget {
    Remaining(u64),
    Covered,
    Unbounded,
}

impl Budget {
    fn toward(target: u64) -> Self {
        Budget::Remaining(target).less(0)
    }

    fn less(self, value: u64) -> Self {
        match self {
            Budget::Remaining(still) if value < still => Budget::Remaining(still - value),
            Budget::Remaining(_) => Budget::Covered,
            Budget::Covered | Budget::Unbounded => self,
        }
    }
}

#[derive(Clone, Copy)]
enum Stage {
    CertainUnreserved,
    AnyUnreserved,
    CertainReserved,
    AnyReserved,
}

const BUDGETED_STAGES: [Stage; 4] = [
    Stage::CertainUnreserved,
    Stage::AnyUnreserved,
    Stage::CertainReserved,
    Stage::AnyReserved,
];

const ALL_FUNDS_STAGES: [Stage; 1] = [Stage::CertainReserved];

impl Stage {
    fn admits<N>(self, candidate: &Candidate<N>) -> bool {
        match self {
            Stage::CertainUnreserved => candidate.certain && !candidate.reserved,
            Stage::AnyUnreserved => !candidate.reserved,
            Stage::CertainReserved => candidate.certain,
            Stage::AnyReserved => true,
        }
    }
}

pub(crate) fn select(
    target_value: TargetValue,
    sources: &[ShieldedPool],
    mut candidates: Candidates,
) -> Selection {
    let (stages, mut budget) = match target_value {
        TargetValue::AtLeast(at_least) => {
            (&BUDGETED_STAGES[..], Budget::toward(at_least.into_u64()))
        }
        TargetValue::AllFunds(MaxSpendMode::MaxSpendable) => {
            (&ALL_FUNDS_STAGES[..], Budget::Unbounded)
        }
        TargetValue::AllFunds(MaxSpendMode::Everything) => {
            return Selection::EverythingUnsupported;
        }
    };
    let mut notes = PoolNotes::default();
    for (stage, pool) in passes(stages, sources) {
        budget = pick_pool(pool, stage, &mut candidates, &mut notes, budget);
    }
    match budget {
        Budget::Remaining(_) => Selection::Insufficient { notes },
        Budget::Covered => Selection::Sufficient { notes },
        Budget::Unbounded => Selection::AllSpendable { notes },
    }
}

fn passes<'a>(
    stages: &'a [Stage],
    sources: &'a [ShieldedPool],
) -> impl Iterator<Item = (Stage, ShieldedPool)> + 'a {
    stages
        .iter()
        .flat_map(move |&stage| sources.iter().map(move |&pool| (stage, pool)))
}

fn pick_pool(
    pool: ShieldedPool,
    stage: Stage,
    candidates: &mut Candidates,
    notes: &mut PoolNotes,
    budget: Budget,
) -> Budget {
    match pool {
        ShieldedPool::Sapling => {
            pick_into(&mut candidates.sapling, &mut notes.sapling, budget, stage)
        }
        ShieldedPool::Orchard => {
            pick_into(&mut candidates.orchard, &mut notes.orchard, budget, stage)
        }
        ShieldedPool::Ironwood => {
            pick_into(&mut candidates.ironwood, &mut notes.ironwood, budget, stage)
        }
    }
}

fn pick_into<N: OutputInterface>(
    candidates: &mut Vec<Candidate<N>>,
    notes: &mut Vec<N>,
    budget: Budget,
    stage: Stage,
) -> Budget {
    let pick = pick(std::mem::take(candidates), budget, stage);
    *candidates = pick.left;
    notes.extend(pick.taken);
    pick.budget
}

struct Pick<N> {
    taken: Vec<N>,
    left: Vec<Candidate<N>>,
    budget: Budget,
}

fn pick<N: OutputInterface>(
    candidates: Vec<Candidate<N>>,
    mut budget: Budget,
    stage: Stage,
) -> Pick<N> {
    if let Budget::Covered = budget {
        return Pick {
            taken: Vec::new(),
            left: candidates,
            budget,
        };
    }
    let fee = MARGINAL_FEE.into_u64();
    let (admitted, mut left): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|candidate| stage.admits(candidate) && candidate.note.value() > fee);
    let mut admitted = Admitted::of(admitted);
    let mut taken = Vec::new();
    while let Some(candidate) = admitted.take(budget) {
        budget = budget.less(candidate.note.value());
        taken.push(candidate.note);
    }
    left.extend(admitted.0.into_values().flat_map(BTreeMap::into_values));
    Pick {
        taken,
        left,
        budget,
    }
}

struct Admitted<N>(BTreeMap<u64, BTreeMap<OutputId, Candidate<N>>>);

impl<N: OutputInterface> Admitted<N> {
    fn of(candidates: Vec<Candidate<N>>) -> Self {
        let mut by_value = BTreeMap::<u64, BTreeMap<_, _>>::new();
        for candidate in candidates {
            by_value
                .entry(candidate.note.value())
                .or_default()
                .insert(candidate.note.output_id(), candidate);
        }
        Admitted(by_value)
    }

    fn take(&mut self, budget: Budget) -> Option<Candidate<N>> {
        let value = match budget {
            Budget::Covered => return None,
            Budget::Unbounded => *self.0.keys().next()?,
            Budget::Remaining(still) => {
                let covering = still + MARGINAL_FEE.into_u64() + 1;
                match self.0.range(covering..).next() {
                    Some((&value, _)) => value,
                    None => *self.0.keys().next_back()?,
                }
            }
        };
        let mut of_value = self.0.remove(&value)?;
        let (_, candidate) = of_value.pop_first()?;
        if !of_value.is_empty() {
            self.0.insert(value, of_value);
        }
        Some(candidate)
    }
}

#[cfg(test)]
mod tests;
