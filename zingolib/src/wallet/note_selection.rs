use pepper_sync::wallet::{IronwoodNote, OrchardNote, OutputInterface, SaplingNote};
use zcash_client_backend::data_api::{MaxSpendMode, TargetValue};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::ShieldedPool;

const FALLBACK: [ShieldedPool; 3] = [
    ShieldedPool::Sapling,
    ShieldedPool::Orchard,
    ShieldedPool::Ironwood,
];

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

pub(crate) fn select(
    target_value: TargetValue,
    sources: &[ShieldedPool],
    candidates: Candidates,
) -> Selection {
    match target_value {
        TargetValue::AtLeast(at_least) => budgeted(at_least.into_u64(), sources, candidates),
        TargetValue::AllFunds(MaxSpendMode::MaxSpendable) => Selection::AllSpendable {
            notes: PoolNotes {
                sapling: spend_worthy(candidates.sapling),
                orchard: spend_worthy(candidates.orchard),
                ironwood: spend_worthy(candidates.ironwood),
            },
        },
        TargetValue::AllFunds(MaxSpendMode::Everything) => Selection::EverythingUnsupported,
    }
}

fn spend_worthy<N: OutputInterface>(candidates: Vec<Candidate<N>>) -> Vec<N> {
    candidates
        .into_iter()
        .filter(|candidate| candidate.certain && candidate.note.value() > MARGINAL_FEE.into_u64())
        .map(|candidate| candidate.note)
        .collect()
}

fn budgeted(target: u64, sources: &[ShieldedPool], mut candidates: Candidates) -> Selection {
    let mut notes = PoolNotes::default();
    let mut remaining = Remaining::of(target, 0);
    let any_reserved = candidates
        .sapling
        .iter()
        .any(|candidate| candidate.reserved)
        || candidates
            .orchard
            .iter()
            .any(|candidate| candidate.reserved)
        || candidates
            .ironwood
            .iter()
            .any(|candidate| candidate.reserved);
    for release_reserved in [false, true] {
        if release_reserved && !(any_reserved && matches!(remaining, Remaining::Positive(_))) {
            break;
        }
        for include_uncertain in [false, true] {
            for source in sources.iter().chain(&FALLBACK) {
                match source {
                    ShieldedPool::Sapling => pick(
                        &mut candidates.sapling,
                        &mut notes.sapling,
                        &mut remaining,
                        include_uncertain,
                        release_reserved,
                    ),
                    ShieldedPool::Orchard => pick(
                        &mut candidates.orchard,
                        &mut notes.orchard,
                        &mut remaining,
                        include_uncertain,
                        release_reserved,
                    ),
                    ShieldedPool::Ironwood => pick(
                        &mut candidates.ironwood,
                        &mut notes.ironwood,
                        &mut remaining,
                        include_uncertain,
                        release_reserved,
                    ),
                }
            }
        }
    }
    match remaining {
        Remaining::Positive(_) => Selection::Insufficient { notes },
        Remaining::Covered => Selection::Sufficient { notes },
    }
}

fn pick<N: OutputInterface>(
    candidates: &mut Vec<Candidate<N>>,
    selected: &mut Vec<N>,
    remaining: &mut Remaining,
    include_uncertain: bool,
    release_reserved: bool,
) {
    let Remaining::Positive(target) = *remaining else {
        return;
    };
    let fee = MARGINAL_FEE.into_u64();
    let mut eligible = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            (include_uncertain || candidate.certain)
                && (release_reserved || !candidate.reserved)
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
                .note_candidates(anchor_height, account, &[], &[])
                .expect("the synthetic wallet is synced"),
            orchard: wallet
                .note_candidates(anchor_height, account, &[], &[])
                .expect("the synthetic wallet is synced"),
            ironwood: wallet
                .note_candidates(anchor_height, account, &[], &[])
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

    // Tests that the fallback tops up from a pool outside the request when the requested pools fall short.
    #[test]
    fn fallback_tops_up_from_the_other_pools() {
        let Selection::Sufficient { notes } = select(
            at_least(200_000),
            &[ShieldedPool::Ironwood, ShieldedPool::Orchard],
            candidates_of(&funded_wallet()),
        ) else {
            panic!("the three pools hold 220_000 above the dust line");
        };
        assert_eq!(values(&notes.sapling), [50_000]);
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
        assert_eq!(values(&notes.sapling), [50_000]);
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

    // Tests that all-funds selection keeps every certain note above the dust line in every pool.
    #[test]
    fn all_spendable_keeps_every_spend_worthy_note() {
        let Selection::AllSpendable { notes } = select(
            TargetValue::AllFunds(MaxSpendMode::MaxSpendable),
            &[ShieldedPool::Orchard],
            candidates_of(&funded_wallet()),
        ) else {
            panic!("all funds in MaxSpendable mode is supported");
        };
        assert_eq!(values(&notes.sapling), [50_000]);
        assert_eq!(values(&notes.orchard), [100_000]);
        assert_eq!(values(&notes.ironwood), [70_000]);
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
}
