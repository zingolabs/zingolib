use pepper_sync::wallet::{IronwoodNote, OrchardNote, OutputInterface, SaplingNote};
use zcash_client_backend::data_api::locking::{LockFilter, LockOwner};
use zcash_client_backend::data_api::wallet::input_selection::LockedInputPolicy;
use zcash_client_backend::data_api::{MaxSpendMode, TargetValue};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::ShieldedPool;
use zcash_protocol::value::MAX_MONEY;

pub(crate) struct Candidate<N> {
    pub(crate) note: N,
    pub(crate) certain: bool,
    pub(crate) lock: Option<LockOwner>,
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
enum Remaining {
    Positive(u64),
    Covered,
}

impl Remaining {
    fn of(target: u64, selected: u64) -> Self {
        if selected < target {
            Remaining::Positive(target - selected)
        } else {
            Remaining::Covered
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Preferred,
    Deferred,
    Barred,
}

fn standing(lock_filter: LockFilter<'_>, lock: Option<LockOwner>) -> Standing {
    let LockFilter::Policy(policy) = lock_filter else {
        return Standing::Preferred;
    };
    match (policy, lock) {
        (LockedInputPolicy::PreferLocked(_), None) => Standing::Deferred,
        (_, None) => Standing::Preferred,
        (LockedInputPolicy::PreferUnlocked(owners), Some(owner))
            if owners.as_set().contains(&owner) =>
        {
            Standing::Deferred
        }
        (LockedInputPolicy::PreferLocked(owners), Some(owner))
            if owners.as_set().contains(&owner) =>
        {
            Standing::Preferred
        }
        (_, Some(_)) => Standing::Barred,
    }
}

#[derive(Clone, Copy)]
enum Stage {
    CertainPreferred,
    AnyPreferred,
    CertainDeferred,
    AnyDeferred,
}

const BUDGETED_STAGES: [Stage; 4] = [
    Stage::CertainPreferred,
    Stage::AnyPreferred,
    Stage::CertainDeferred,
    Stage::AnyDeferred,
];

const ALL_FUNDS_STAGES: [Stage; 2] = [Stage::CertainPreferred, Stage::CertainDeferred];

impl Stage {
    fn admits(self, certain: bool, standing: Standing) -> bool {
        match self {
            Stage::CertainPreferred => certain && standing == Standing::Preferred,
            Stage::AnyPreferred => standing == Standing::Preferred,
            Stage::CertainDeferred => certain && standing != Standing::Barred,
            Stage::AnyDeferred => standing != Standing::Barred,
        }
    }
}

pub(crate) fn select(
    target_value: TargetValue,
    sources: &[ShieldedPool],
    lock_filter: LockFilter<'_>,
    mut candidates: Candidates,
) -> Selection {
    let mut notes = PoolNotes::default();
    match target_value {
        TargetValue::AtLeast(at_least) => {
            let mut remaining = Remaining::of(at_least.into_u64(), 0);
            for (stage, pool) in passes(&BUDGETED_STAGES, sources) {
                pick_pool(
                    pool,
                    stage,
                    lock_filter,
                    &mut candidates,
                    &mut notes,
                    &mut remaining,
                );
            }
            match remaining {
                Remaining::Positive(_) => Selection::Insufficient { notes },
                Remaining::Covered => Selection::Sufficient { notes },
            }
        }
        TargetValue::AllFunds(MaxSpendMode::MaxSpendable) => {
            let mut remaining = Remaining::Positive(MAX_MONEY);
            for (stage, pool) in passes(&ALL_FUNDS_STAGES, sources) {
                pick_pool(
                    pool,
                    stage,
                    lock_filter,
                    &mut candidates,
                    &mut notes,
                    &mut remaining,
                );
            }
            Selection::AllSpendable { notes }
        }
        TargetValue::AllFunds(MaxSpendMode::Everything) => Selection::EverythingUnsupported,
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
    lock_filter: LockFilter<'_>,
    candidates: &mut Candidates,
    notes: &mut PoolNotes,
    remaining: &mut Remaining,
) {
    match pool {
        ShieldedPool::Sapling => pick(
            &mut candidates.sapling,
            &mut notes.sapling,
            remaining,
            stage,
            lock_filter,
        ),
        ShieldedPool::Orchard => pick(
            &mut candidates.orchard,
            &mut notes.orchard,
            remaining,
            stage,
            lock_filter,
        ),
        ShieldedPool::Ironwood => pick(
            &mut candidates.ironwood,
            &mut notes.ironwood,
            remaining,
            stage,
            lock_filter,
        ),
    }
}

fn pick<N: OutputInterface>(
    candidates: &mut Vec<Candidate<N>>,
    selected: &mut Vec<N>,
    remaining: &mut Remaining,
    stage: Stage,
    lock_filter: LockFilter<'_>,
) {
    let Remaining::Positive(target) = *remaining else {
        return;
    };
    let fee = MARGINAL_FEE.into_u64();
    let mut eligible = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            stage.admits(candidate.certain, standing(lock_filter, candidate.lock))
                && candidate.note.value() > fee
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let value_at = |index: &usize| candidates[*index].note.value();
    eligible.sort_by_key(value_at);
    let mut picked = Vec::new();
    let mut cursor = 0;
    loop {
        let total = picked.iter().map(value_at).sum::<u64>();
        *remaining = Remaining::of(target, total);
        let Remaining::Positive(still) = *remaining else {
            break;
        };
        match eligible.get(cursor) {
            Some(&index) if value_at(&index) > still + fee => {
                picked.push(index);
                eligible.remove(cursor);
            }
            Some(_) => cursor += 1,
            None => match eligible.pop() {
                Some(index) => {
                    picked.push(index);
                    cursor = 0;
                }
                None => break,
            },
        }
    }
    let mut slots = candidates.drain(..).map(Some).collect::<Vec<_>>();
    selected.extend(
        picked
            .iter()
            .filter_map(|&index| slots[index].take())
            .map(|candidate| candidate.note),
    );
    candidates.extend(slots.into_iter().flatten());
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::data_api::wallet::input_selection::NonEmptyBTreeSet;
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::value::Zatoshis;

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;

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
                .note_candidates(anchor_height, anchor_height + 1, account, &[], &[])
                .expect("the synthetic wallet is synced"),
            orchard: wallet
                .note_candidates(anchor_height, anchor_height + 1, account, &[], &[])
                .expect("the synthetic wallet is synced"),
            ironwood: wallet
                .note_candidates(anchor_height, anchor_height + 1, account, &[], &[])
                .expect("the synthetic wallet is synced"),
        }
    }

    fn select_unfiltered(
        target_value: TargetValue,
        sources: &[ShieldedPool],
        candidates: Candidates,
    ) -> Selection {
        select(target_value, sources, LockFilter::Unfiltered, candidates)
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
        let Selection::Sufficient { notes } = select_unfiltered(
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
        let Selection::Insufficient { notes } = select_unfiltered(
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
        let Selection::Insufficient { notes } = select_unfiltered(
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

    const OWNER: LockOwner = LockOwner::new([7; 32]);
    const OTHER: LockOwner = LockOwner::new([9; 32]);

    fn lock_orchard_note(candidates: &mut Candidates, value: u64, owner: LockOwner) {
        for candidate in &mut candidates.orchard {
            if candidate.note.value() == value {
                candidate.lock = Some(owner);
            }
        }
    }

    // Tests that a note locked by a preferred-unlocked owner is spent only when the free notes fall short.
    #[test]
    fn deferred_notes_are_spent_only_when_preferred_notes_fall_short() {
        let policy = LockedInputPolicy::PreferUnlocked(NonEmptyBTreeSet::singleton(OWNER));
        let mut candidates = candidates_of(&funded_wallet());
        lock_orchard_note(&mut candidates, 100_000, OWNER);
        let Selection::Sufficient { notes } = select(
            at_least(60_000),
            &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
            LockFilter::Policy(&policy),
            candidates,
        ) else {
            panic!("ironwood covers 60_000 without the deferred note");
        };
        assert!(notes.orchard.is_empty());
        assert_eq!(values(&notes.ironwood), [70_000]);

        let mut candidates = candidates_of(&funded_wallet());
        lock_orchard_note(&mut candidates, 100_000, OWNER);
        let Selection::Sufficient { notes } = select(
            at_least(150_000),
            &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
            LockFilter::Policy(&policy),
            candidates,
        ) else {
            panic!("the deferred note is released once the others fall short");
        };
        assert_eq!(values(&notes.orchard), [100_000]);
        assert_eq!(values(&notes.ironwood), [70_000]);
    }

    // Tests that a note locked by another owner is never spent, even when the request then falls short.
    #[test]
    fn barred_notes_are_never_spent() {
        let policy = LockedInputPolicy::PreferUnlocked(NonEmptyBTreeSet::singleton(OWNER));
        let mut candidates = candidates_of(&funded_wallet());
        lock_orchard_note(&mut candidates, 100_000, OTHER);
        let Selection::Insufficient { notes } = select(
            at_least(150_000),
            &[ShieldedPool::Orchard, ShieldedPool::Ironwood],
            LockFilter::Policy(&policy),
            candidates,
        ) else {
            panic!("ironwood alone holds 70_000");
        };
        assert!(notes.orchard.is_empty());
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
        let Selection::Sufficient { notes } = select_unfiltered(
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
        let Selection::Sufficient { notes } = select_unfiltered(
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
        let Selection::AllSpendable { notes } = select_unfiltered(
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
            select_unfiltered(
                TargetValue::AllFunds(MaxSpendMode::Everything),
                &ALL_POOLS,
                candidates_of(&funded_wallet()),
            ),
            Selection::EverythingUnsupported
        ));
    }
}
