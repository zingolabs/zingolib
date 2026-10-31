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
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::value::Zatoshis;

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::output::OutputRef;

    const ALL_POOLS: [ShieldedPool; 3] = [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ];

    fn funded_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .sapling_note(50_000)
            .orchard_note(100_000)
            .ironwood_note(70_000)
            .orchard_note(5_000)
            .build()
    }

    fn candidates_of(wallet: &LightWallet) -> Candidates {
        let anchor_height = BlockHeight::from_u32(20);
        let account = zip32::AccountId::ZERO;
        Candidates {
            sapling: wallet
                .note_candidates(anchor_height, account, &[], &HashSet::new())
                .expect("the synthetic wallet is synced"),
            orchard: wallet
                .note_candidates(anchor_height, account, &[], &HashSet::new())
                .expect("the synthetic wallet is synced"),
            ironwood: wallet
                .note_candidates(anchor_height, account, &[], &HashSet::new())
                .expect("the synthetic wallet is synced"),
        }
    }

    fn at_least(value: u64) -> TargetValue {
        TargetValue::AtLeast(Zatoshis::const_from_u64(value))
    }

    fn values<N: OutputInterface>(notes: &[N]) -> Vec<u64> {
        notes.iter().map(OutputInterface::value).collect()
    }

    // Tests that a covered target selects from the requested pools in their order when they hold enough value.
    #[test]
    fn covered_target_selects_from_the_requested_pools() {
        let Selection::Sufficient { notes } = select(
            at_least(120_000),
            &ALL_POOLS,
            candidates_of(&funded_wallet()),
        ) else {
            panic!("the funded wallet covers 120_000");
        };
        assert_eq!(values(&notes.sapling), [50_000]);
        assert_eq!(values(&notes.orchard), [100_000]);
        assert!(notes.ironwood.is_empty());
    }

    // Tests that selection stays in the requested pools when they fall short.
    #[test]
    fn uncovered_request_stays_in_the_requested_pools() {
        let Selection::Insufficient { notes } = select(
            at_least(200_000),
            &[ShieldedPool::Ironwood, ShieldedPool::Orchard],
            candidates_of(&funded_wallet()),
        ) else {
            panic!("ironwood and orchard hold 170_000 above the dust line");
        };
        assert!(notes.sapling.is_empty());
        assert_eq!(values(&notes.orchard), [100_000]);
        assert_eq!(values(&notes.ironwood), [70_000]);
    }

    // Tests that an uncovered target returns every note above the dust line when the wallet falls short.
    #[test]
    fn uncovered_target_returns_every_spend_worthy_note() {
        let Selection::Insufficient { notes } = select(
            at_least(300_000),
            &ALL_POOLS,
            candidates_of(&funded_wallet()),
        ) else {
            panic!("the funded wallet holds 220_000 above the dust line");
        };
        assert_eq!(values(&notes.sapling), [50_000]);
        assert_eq!(values(&notes.orchard), [100_000]);
        assert_eq!(values(&notes.ironwood), [70_000]);
    }

    // Tests that a reserved note is spent only when the unreserved notes fall short.
    #[test]
    fn reserved_notes_are_withheld_until_needed() {
        let mut candidates = candidates_of(&funded_wallet());
        for candidate in &mut candidates.orchard {
            candidate.reserved = candidate.note.value() == 100_000;
        }
        let Selection::Sufficient { notes } = select(
            at_least(60_000),
            &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
            candidates,
        ) else {
            panic!("ironwood covers 60_000 without the reserved note");
        };
        assert!(notes.orchard.is_empty());
        assert_eq!(values(&notes.ironwood), [70_000]);

        let mut candidates = candidates_of(&funded_wallet());
        for candidate in &mut candidates.orchard {
            candidate.reserved = candidate.note.value() == 100_000;
        }
        let Selection::Sufficient { notes } = select(
            at_least(150_000),
            &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
            candidates,
        ) else {
            panic!("the reserved note is released once the others fall short");
        };
        assert!(notes.sapling.is_empty());
        assert_eq!(values(&notes.orchard), [100_000]);
        assert_eq!(values(&notes.ironwood), [70_000]);
    }

    // Tests that an uncertain note is spent only when the certain notes fall short.
    #[test]
    fn uncertain_notes_are_spent_last() {
        let mut candidates = candidates_of(&funded_wallet());
        for candidate in &mut candidates.sapling {
            candidate.certain = false;
        }
        for candidate in &mut candidates.orchard {
            candidate.certain = false;
        }
        let Selection::Sufficient { notes } = select(
            at_least(60_000),
            &[ShieldedPool::Sapling, ShieldedPool::Ironwood],
            candidates,
        ) else {
            panic!("ironwood covers 60_000 without an uncertain note");
        };
        assert!(notes.sapling.is_empty());
        assert!(notes.orchard.is_empty());
        assert_eq!(values(&notes.ironwood), [70_000]);

        let mut candidates = candidates_of(&funded_wallet());
        for candidate in &mut candidates.sapling {
            candidate.certain = false;
        }
        for candidate in &mut candidates.orchard {
            candidate.certain = false;
        }
        let Selection::Sufficient { notes } = select(
            at_least(100_000),
            &[ShieldedPool::Sapling, ShieldedPool::Ironwood],
            candidates,
        ) else {
            panic!("the uncertain sapling note completes the target once ironwood falls short");
        };
        assert_eq!(values(&notes.sapling), [50_000]);
        assert!(notes.orchard.is_empty());
        assert_eq!(values(&notes.ironwood), [70_000]);
    }

    // Tests that all-funds selection keeps the certain notes above the dust line of the requested pools only.
    #[test]
    fn all_spendable_keeps_the_requested_pools_only() {
        let Selection::AllSpendable { notes } = select(
            TargetValue::AllFunds(MaxSpendMode::MaxSpendable),
            &[ShieldedPool::Orchard],
            candidates_of(&funded_wallet()),
        ) else {
            panic!("all funds in MaxSpendable mode is supported");
        };
        assert!(notes.sapling.is_empty());
        assert_eq!(values(&notes.orchard), [100_000]);
        assert!(notes.ironwood.is_empty());
    }

    // Tests that all-funds selection in Everything mode is refused.
    #[test]
    fn everything_is_refused() {
        assert!(matches!(
            select(
                TargetValue::AllFunds(MaxSpendMode::Everything),
                &ALL_POOLS,
                candidates_of(&funded_wallet()),
            ),
            Selection::EverythingUnsupported
        ));
    }

    fn ladder_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(10_000)
            .orchard_note(30_000)
            .orchard_note(90_000)
            .build()
    }

    fn all_funds() -> TargetValue {
        TargetValue::AllFunds(MaxSpendMode::MaxSpendable)
    }

    fn sorted_values<N: OutputInterface>(notes: &[N]) -> Vec<u64> {
        let mut values = values(notes);
        values.sort_unstable();
        values
    }

    fn all_values(notes: &PoolNotes) -> Vec<u64> {
        [
            values(&notes.sapling),
            values(&notes.orchard),
            values(&notes.ironwood),
        ]
        .concat()
    }

    fn ids<N: OutputInterface>(notes: &[N]) -> BTreeSet<OutputId> {
        notes.iter().map(OutputInterface::output_id).collect()
    }

    fn candidate_ids<N: OutputInterface>(candidates: &[Candidate<N>]) -> BTreeSet<OutputId> {
        candidates
            .iter()
            .map(|candidate| candidate.note.output_id())
            .collect()
    }

    #[test]
    fn budgeted_pick_prefers_the_smallest_single_covering_note() {
        let Selection::Sufficient { notes } = select(
            at_least(20_000),
            &[ShieldedPool::Orchard],
            candidates_of(&ladder_wallet()),
        ) else {
            panic!("the ladder wallet covers 20_000");
        };
        assert_eq!(values(&notes.orchard), [30_000]);
    }

    #[test]
    fn budgeted_pick_falls_back_to_the_largest_note_then_covers_the_rest() {
        let target = 100_000;
        let Selection::Sufficient { notes } = select(
            at_least(target),
            &[ShieldedPool::Orchard],
            candidates_of(&ladder_wallet()),
        ) else {
            panic!("the ladder wallet covers 100_000");
        };
        assert_eq!(sorted_values(&notes.orchard), [30_000, 90_000]);
        assert!(all_values(&notes).iter().sum::<u64>() >= target);
    }

    #[test]
    fn pick_conserves_the_candidates() {
        let candidates = candidates_of(&ladder_wallet()).orchard;
        let before = candidate_ids(&candidates);
        let pick = pick(candidates, Budget::toward(100_000), Stage::AnyReserved);
        let taken = ids(&pick.taken);
        let left = candidate_ids(&pick.left);
        assert_eq!(taken.len() + left.len(), before.len());
        assert!(taken.is_disjoint(&left));
        assert_eq!(taken.union(&left).copied().collect::<BTreeSet<_>>(), before);
        assert!(matches!(pick.budget, Budget::Covered));
    }

    #[test]
    fn covered_budget_picks_nothing() {
        let candidates = candidates_of(&ladder_wallet()).orchard;
        let before = candidate_ids(&candidates);
        let pick = pick(candidates, Budget::Covered, Stage::AnyReserved);
        assert!(pick.taken.is_empty());
        assert_eq!(candidate_ids(&pick.left), before);
        assert!(matches!(pick.budget, Budget::Covered));
    }

    #[test]
    fn unbounded_pick_takes_every_admitted_note_by_rising_value() {
        let candidates = candidates_of(&ladder_wallet()).orchard;
        let pick = pick(candidates, Budget::Unbounded, Stage::AnyReserved);
        assert_eq!(values(&pick.taken), [10_000, 30_000, 90_000]);
        assert!(pick.left.is_empty());
        assert!(matches!(pick.budget, Budget::Unbounded));
    }

    #[test]
    fn no_selected_note_sits_at_or_below_the_dust_line() {
        let dust = MARGINAL_FEE.into_u64();
        let Selection::Insufficient { notes } = select(
            at_least(1_000_000),
            &ALL_POOLS,
            candidates_of(&funded_wallet()),
        ) else {
            panic!("the funded wallet holds 220_000 above the dust line");
        };
        assert!(all_values(&notes).iter().all(|&value| value > dust));
        let Selection::AllSpendable { notes } =
            select(all_funds(), &ALL_POOLS, candidates_of(&funded_wallet()))
        else {
            panic!("all funds in MaxSpendable mode is supported");
        };
        assert!(all_values(&notes).iter().all(|&value| value > dust));
    }

    #[test]
    fn selection_is_independent_of_candidate_order() {
        let wallet = ladder_wallet();
        let forward = candidates_of(&wallet);
        let mut reversed = candidates_of(&wallet);
        reversed.orchard.reverse();
        let (
            Selection::AllSpendable {
                notes: from_forward,
            },
            Selection::AllSpendable {
                notes: from_reversed,
            },
        ) = (
            select(all_funds(), &[ShieldedPool::Orchard], forward),
            select(all_funds(), &[ShieldedPool::Orchard], reversed),
        )
        else {
            panic!("all funds in MaxSpendable mode is supported");
        };
        assert_eq!(
            values(&from_forward.orchard),
            values(&from_reversed.orchard)
        );

        let forward = candidates_of(&wallet);
        let mut reversed = candidates_of(&wallet);
        reversed.orchard.reverse();
        let (
            Selection::Sufficient {
                notes: from_forward,
            },
            Selection::Sufficient {
                notes: from_reversed,
            },
        ) = (
            select(at_least(100_000), &[ShieldedPool::Orchard], forward),
            select(at_least(100_000), &[ShieldedPool::Orchard], reversed),
        )
        else {
            panic!("the ladder wallet covers 100_000");
        };
        assert_eq!(
            values(&from_forward.orchard),
            values(&from_reversed.orchard)
        );
    }

    #[test]
    fn note_candidates_honour_exclusions_and_reservations() {
        let wallet = funded_wallet();
        let orchard = candidates_of(&wallet).orchard;
        let id_of = |value: u64| {
            orchard
                .iter()
                .find(|candidate| candidate.note.value() == value)
                .map(|candidate| candidate.note.output_id())
                .expect("the funded wallet holds an orchard note of this value")
        };
        let excluded = id_of(100_000);
        let reserved = id_of(5_000);
        let candidates: Vec<Candidate<OrchardNote>> = wallet
            .note_candidates(
                BlockHeight::from_u32(20),
                zip32::AccountId::ZERO,
                &[OutputRef::new(excluded, OrchardNote::POOL_TYPE)],
                &HashSet::from([reserved]),
            )
            .expect("the synthetic wallet is synced");
        assert_eq!(candidate_ids(&candidates), BTreeSet::from([reserved]));
        assert!(candidates.iter().all(|candidate| candidate.certain));
        assert!(candidates.iter().all(|candidate| candidate.reserved));
    }
}
