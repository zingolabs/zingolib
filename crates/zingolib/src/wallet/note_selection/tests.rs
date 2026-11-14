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

fn twin_wallet() -> LightWallet {
    SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .tip(20)
        .orchard_note(30_000)
        .orchard_note(30_000)
        .build()
}

fn taken_ids<N: OutputInterface>(notes: &[N]) -> Vec<OutputId> {
    notes.iter().map(OutputInterface::output_id).collect()
}

#[test]
fn equal_values_are_taken_one_each_by_rising_output_id() {
    let candidates = candidates_of(&twin_wallet()).orchard;
    let by_id: Vec<OutputId> = candidate_ids(&candidates).into_iter().collect();
    assert_eq!(by_id.len(), 2);

    let unbounded = pick(candidates, Budget::Unbounded, Stage::AnyReserved);
    assert_eq!(taken_ids(&unbounded.taken), by_id);
    assert!(unbounded.left.is_empty());

    let candidates = candidates_of(&twin_wallet()).orchard;
    let budgeted = pick(candidates, Budget::toward(20_000), Stage::AnyReserved);
    assert_eq!(taken_ids(&budgeted.taken), by_id[..1]);
    assert_eq!(candidate_ids(&budgeted.left), BTreeSet::from([by_id[1]]));
    assert!(matches!(budgeted.budget, Budget::Covered));
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
