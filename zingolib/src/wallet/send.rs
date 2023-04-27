use orchard::note_encryption::OrchardDomain;
use zcash_primitives::{sapling::note_encryption::SaplingDomain, transaction::components::Amount};

use super::{
    data::{SpendableOrchardNote, SpendableSaplingNote, Utxo},
    LightWallet, NoteSelectionError, PoolTargets,
};

impl LightWallet {
    pub(super) async fn select_notes_full_privacy(
        &self,
        PoolTargets {
            target_transparent,
            target_sapling,
            target_orchard,
        }: PoolTargets,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            Amount,
        ),
        NoteSelectionError,
    > {
        if target_transparent != Amount::zero() {
            Err(NoteSelectionError::DisallowedByPrivacyPolicy)
        } else {
            let sapling_candidates = self
                .get_spendable_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
                .await;
            let (sapling_notes, sapling_value_selected) = Self::add_notes_to_total::<
                SaplingDomain<zingoconfig::ChainType>,
            >(
                sapling_candidates, target_sapling
            );
            let orchard_candidates = self
                .get_spendable_domain_specific_notes::<OrchardDomain>()
                .await;
            let (orchard_notes, orchard_value_selected) =
                Self::add_notes_to_total::<OrchardDomain>(orchard_candidates, target_orchard);

            if orchard_value_selected >= target_orchard && sapling_value_selected >= target_sapling
            {
                Ok((
                    orchard_notes,
                    sapling_notes,
                    Vec::new(), // No utxos for full privacy
                    (sapling_value_selected + orchard_value_selected).unwrap(),
                ))
            } else {
                Err(NoteSelectionError::InsufficientPrivateFunds)
            }
        }
    }
    pub(super) async fn select_notes_revealed_amounts(
        &self,
        PoolTargets {
            target_transparent,
            target_sapling,
            target_orchard,
        }: PoolTargets,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            Amount,
        ),
        NoteSelectionError,
    > {
        if target_transparent != Amount::zero() {
            Err(NoteSelectionError::DisallowedByPrivacyPolicy)
        } else {
            let full_target = (target_sapling + target_orchard).unwrap();
            let sapling_candidates = self
                .get_spendable_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
                .await;
            let (sapling_notes, sapling_value_selected) = Self::add_notes_to_total::<
                SaplingDomain<zingoconfig::ChainType>,
            >(
                sapling_candidates, full_target
            );
            if sapling_value_selected >= full_target {
                return Ok((
                    Vec::new(),
                    sapling_notes,
                    Vec::new(),
                    sapling_value_selected,
                ));
            }
            let orchard_candidates = self
                .get_spendable_domain_specific_notes::<OrchardDomain>()
                .await;
            let (orchard_notes, orchard_value_selected) = Self::add_notes_to_total::<OrchardDomain>(
                orchard_candidates,
                (full_target - sapling_value_selected).unwrap(),
            );
            Ok((
                orchard_notes,
                sapling_notes,
                Vec::new(),
                (sapling_value_selected + orchard_value_selected).unwrap(),
            ))
        }
    }
}
