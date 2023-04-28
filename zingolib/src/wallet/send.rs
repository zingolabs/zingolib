use orchard::note_encryption::OrchardDomain;
use zcash_primitives::{sapling::note_encryption::SaplingDomain, transaction::components::Amount};

use super::{
    data::{SpendableOrchardNote, SpendableSaplingNote, Utxo},
    DomainSpecificAmounts, LightWallet, NoteSelectionError,
};

impl LightWallet {
    pub(super) async fn select_notes_full_privacy(
        &self,
        DomainSpecificAmounts {
            transparent: target_transparent,
            sapling: target_sapling,
            orchard: target_orchard,
        }: DomainSpecificAmounts,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            DomainSpecificAmounts,
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
                    DomainSpecificAmounts {
                        sapling: sapling_value_selected,
                        orchard: orchard_value_selected,
                        transparent: Amount::zero(),
                    },
                ))
            } else {
                Err(NoteSelectionError::InsufficientPrivateFunds)
            }
        }
    }
    pub(super) async fn select_notes_revealed_amounts(
        &self,
        pool_targets: DomainSpecificAmounts,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            DomainSpecificAmounts,
        ),
        NoteSelectionError,
    > {
        if pool_targets.transparent != Amount::zero() {
            Err(NoteSelectionError::DisallowedByPrivacyPolicy)
        } else {
            // We don't select notes any differently, we just enforce no shielded recipients
            self.select_notes_revealed_recipients(pool_targets).await
        }
    }
    pub(super) async fn select_notes_revealed_recipients(
        &self,
        target_amounts: DomainSpecificAmounts,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            DomainSpecificAmounts,
        ),
        NoteSelectionError,
    > {
        let mut selected_amounts = DomainSpecificAmounts::default();
        let sapling_candidates = self
            .get_spendable_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
            .await;
        let (sapling_notes, sapling_value_selected) = Self::add_notes_to_total::<
            SaplingDomain<zingoconfig::ChainType>,
        >(
            sapling_candidates, target_amounts.total()
        );
        selected_amounts.sapling = sapling_value_selected;
        if selected_amounts.total() >= target_amounts.total() {
            return Ok((Vec::new(), sapling_notes, Vec::new(), selected_amounts));
        }
        let orchard_candidates = self
            .get_spendable_domain_specific_notes::<OrchardDomain>()
            .await;
        let (orchard_notes, orchard_value_selected) = Self::add_notes_to_total::<OrchardDomain>(
            orchard_candidates,
            (target_amounts.total() - sapling_value_selected).unwrap(),
        );
        selected_amounts.orchard = orchard_value_selected;
        if selected_amounts.total() >= target_amounts.total() {
            Ok((orchard_notes, sapling_notes, Vec::new(), selected_amounts))
        } else if (selected_amounts.total()
            + self
                .unspent_utxos()
                .await
                .iter()
                .fold(Amount::zero(), |prev, utxo| {
                    (prev + Amount::from_u64(utxo.value).unwrap()).unwrap()
                }))
        .unwrap()
            >= target_amounts.total()
        {
            Err(NoteSelectionError::InsufficientPrivateFunds)
        } else {
            Err(NoteSelectionError::InsufficientSpendableFunds)
        }
    }

    pub(super) async fn select_notes_revealed_senders(
        &self,
        target_amounts: DomainSpecificAmounts,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            DomainSpecificAmounts,
        ),
        NoteSelectionError,
    > {
        if target_amounts.transparent != Amount::zero() {
            Err(NoteSelectionError::DisallowedByPrivacyPolicy)
        } else {
            // We use the same logic, just with transparent recipients disallowed
            self.select_notes_fully_transparent(target_amounts).await
        }
    }

    pub(super) async fn select_notes_fully_transparent(
        &self,
        target_amounts: DomainSpecificAmounts,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<Utxo>,
            DomainSpecificAmounts,
        ),
        NoteSelectionError,
    > {
        let mut selected_amounts = DomainSpecificAmounts::default();
        let utxos = self
            .get_utxos()
            .await
            .iter()
            .filter(|utxo| utxo.unconfirmed_spent.is_none() && utxo.spent.is_none())
            .cloned()
            .collect::<Vec<_>>();
        selected_amounts.transparent = utxos.iter().fold(Amount::zero(), |prev, utxo| {
            (prev + Amount::from_u64(utxo.value).unwrap()).unwrap()
        });
        if selected_amounts.transparent >= target_amounts.total() {
            return Ok((Vec::new(), Vec::new(), utxos, selected_amounts));
        }

        let sapling_candidates = self
            .get_spendable_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
            .await;
        let (sapling_notes, sapling_value_selected) =
            Self::add_notes_to_total::<SaplingDomain<zingoconfig::ChainType>>(
                sapling_candidates,
                (target_amounts.total() - selected_amounts.total()).unwrap(),
            );
        selected_amounts.sapling = sapling_value_selected;
        if selected_amounts.total() >= target_amounts.total() {
            return Ok((Vec::new(), sapling_notes, utxos, selected_amounts));
        }

        let orchard_candidates = self
            .get_spendable_domain_specific_notes::<OrchardDomain>()
            .await;
        let (orchard_notes, orchard_value_selected) = Self::add_notes_to_total::<OrchardDomain>(
            orchard_candidates,
            (target_amounts.total() - selected_amounts.total()).unwrap(),
        );
        selected_amounts.orchard = orchard_value_selected;
        if selected_amounts.total() >= target_amounts.total() {
            return Ok((orchard_notes, sapling_notes, utxos, selected_amounts));
        } else {
            Err(NoteSelectionError::InsufficientSpendableFunds)
        }
    }

    async fn unspent_utxos(&self) -> Vec<Utxo> {
        self.get_utxos()
            .await
            .iter()
            .filter(|utxo| utxo.unconfirmed_spent.is_none() && utxo.spent.is_none())
            .cloned()
            .collect()
    }
}
