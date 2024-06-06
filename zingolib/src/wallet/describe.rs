//! Wallet-State reporters as LightWallet methods.
use orchard::note_encryption::OrchardDomain;

use sapling_crypto::note_encryption::SaplingDomain;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use std::{cmp, sync::Arc};
use tokio::sync::RwLock;
use zcash_primitives::zip339::Mnemonic;

use zcash_note_encryption::Domain;

use zcash_primitives::consensus::BlockHeight;

use crate::wallet::data::TransactionRecord;
use crate::wallet::notes::OutputInterface;
use crate::wallet::notes::ShieldedNoteInterface;

use crate::wallet::traits::Diversifiable as _;

use crate::wallet::keys::unified::{Capability, WalletCapability};
use crate::wallet::notes::TransparentOutput;
use crate::wallet::traits::DomainWalletExt;
use crate::wallet::traits::Recipient;

use crate::wallet::{data::BlockData, tx_map_and_maybe_trees::TxMapAndMaybeTrees};

use crate::wallet::LightWallet;
impl LightWallet {
    /// TODO: Add Doc Comment Here!
    #[allow(clippy::type_complexity)]
    pub async fn shielded_balance<D>(
        &self,
        filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool + '_>],
    ) -> Option<u64>
    where
        D: DomainWalletExt,
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        Some(
            self.transaction_context
                .transaction_metadata_set
                .read()
                .await
                .transaction_records_by_id
                .values()
                .map(|transaction| {
                    let mut unfiltered_notes: Box<dyn Iterator<Item = &D::WalletNote>> =
                        Box::new(D::WalletNote::transaction_metadata_notes(transaction).iter());
                    // All filters in iterator are applied, by this loop
                    for filtering_fn in filters {
                        unfiltered_notes = Box::new(
                            unfiltered_notes.filter(|nnmd| filtering_fn(nnmd, transaction)),
                        )
                    }
                    unfiltered_notes
                        .map(|notedata| {
                            if notedata.spent().is_none() && notedata.pending_spent().is_none() {
                                <D::WalletNote as OutputInterface>::value(notedata)
                            } else {
                                0
                            }
                        })
                        .sum::<u64>()
                })
                .sum::<u64>(),
        )
    }

    /// TODO: Add Doc Comment Here!
    // TODO: this should minus the fee of sending the confirmed balance!
    pub async fn spendable_orchard_balance(&self) -> Option<u64> {
        if let Capability::Spend(_) = self.wallet_capability().orchard {
            self.verified_balance::<OrchardDomain>().await
        } else {
            None
        }
    }

    /// TODO: Add Doc Comment Here!
    // TODO: this should minus the fee of sending the confirmed balance!
    pub async fn spendable_sapling_balance(&self) -> Option<u64> {
        if let Capability::Spend(_) = self.wallet_capability().sapling {
            self.verified_balance::<SaplingDomain>().await
        } else {
            None
        }
    }

    /// TODO: Add Doc Comment Here!
    pub async fn tbalance(&self, addr: Option<String>) -> Option<u64> {
        if self.wallet_capability().transparent.can_view() {
            Some(
                self.get_utxos()
                    .await
                    .iter()
                    .filter(|utxo| match addr.as_ref() {
                        Some(a) => utxo.address == *a,
                        None => true,
                    })
                    .map(|utxo| utxo.value)
                    .sum::<u64>(),
            )
        } else {
            None
        }
    }

    /// TODO: Add Doc Comment Here!
    pub async fn unverified_balance<D: DomainWalletExt>(&self) -> Option<u64>
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        let anchor_height = self.get_anchor_height().await;
        #[allow(clippy::type_complexity)]
        let filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool>] =
            &[Box::new(|nnmd, transaction| {
                !transaction
                    .status
                    .is_confirmed_before_or_at(&BlockHeight::from_u32(anchor_height))
                    || nnmd.pending_receipt()
            })];
        self.shielded_balance::<D>(filters).await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn verified_balance<D: DomainWalletExt>(&self) -> Option<u64>
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        let anchor_height = self.get_anchor_height().await;
        #[allow(clippy::type_complexity)]
        let filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool>] = &[
            Box::new(|_, transaction| {
                transaction
                    .status
                    .is_confirmed_before_or_at(&BlockHeight::from_u32(anchor_height))
            }),
            Box::new(|nnmd, _| !nnmd.pending_receipt()),
        ];
        self.shielded_balance::<D>(filters).await
    }

    /// Returns balance for a given shielded pool excluding any notes with value less than marginal fee
    /// that are confirmed on the block chain (the block has at least 1 confirmation)
    pub async fn confirmed_balance_excluding_dust<D: DomainWalletExt>(&self) -> Option<u64>
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        #[allow(clippy::type_complexity)]
        let filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool>] = &[
            Box::new(|_, transaction| transaction.status.is_confirmed()),
            Box::new(|note, _| !note.pending_receipt()),
            Box::new(|note, _| note.value() >= MARGINAL_FEE.into_u64()),
        ];
        self.shielded_balance::<D>(filters).await
    }

    /// Deprecated for `shielded_balance`
    #[deprecated(note = "deprecated for `shielded_balance` as incorrectly named and unnecessary")]
    pub async fn maybe_verified_orchard_balance(&self) -> Option<u64> {
        self.shielded_balance::<OrchardDomain>(&[]).await
    }

    /// Deprecated for `shielded_balance`
    #[deprecated(note = "deprecated for `shielded_balance` as incorrectly named and unnecessary")]
    pub async fn maybe_verified_sapling_balance(&self) -> Option<u64> {
        self.shielded_balance::<SaplingDomain>(&[]).await
    }

    /// TODO: Add Doc Comment Here!
    pub fn wallet_capability(&self) -> Arc<WalletCapability> {
        self.transaction_context.key.clone()
    }

    /// TODO: Add Doc Comment Here!
    pub(crate) fn note_address<D: DomainWalletExt>(
        network: &zingoconfig::ChainType,
        note: &D::WalletNote,
        wallet_capability: &WalletCapability,
    ) -> String
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        D::wc_to_fvk(wallet_capability).expect("to get fvk from wc")
        .diversified_address(*note.diversifier())
        .and_then(|address| {
            D::ua_from_contained_receiver(wallet_capability, &address)
                .map(|ua| ua.encode(network))
        })
        .unwrap_or("Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses".to_string())
    }

    /// TODO: Add Doc Comment Here!
    pub fn mnemonic(&self) -> Option<&(Mnemonic, u32)> {
        self.mnemonic.as_ref()
    }

    /// Get the height of the anchor block
    pub async fn get_anchor_height(&self) -> u32 {
        match self.get_target_height_and_anchor_offset().await {
            Some((height, anchor_offset)) => height - anchor_offset as u32 - 1, // what is the purpose of this -1 ?
            None => 0,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub async fn get_birthday(&self) -> u64 {
        let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
        if birthday == 0 {
            self.get_first_transaction_block().await
        } else {
            cmp::min(self.get_first_transaction_block().await, birthday)
        }
    }

    /// Return a copy of the blocks currently in the wallet, needed to process possible reorgs
    pub async fn get_blocks(&self) -> Vec<BlockData> {
        self.blocks.read().await.iter().cloned().collect()
    }

    /// Get the first block that this wallet has a transaction in. This is often used as the wallet's "birthday"
    /// If there are no transactions, then the actual birthday (which is recorder at wallet creation) is returned
    /// If no birthday was recorded, return the sapling activation height
    pub async fn get_first_transaction_block(&self) -> u64 {
        // Find the first transaction
        let earliest_block = self
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .values()
            .map(|wtx| u64::from(wtx.status.get_height()))
            .min();

        let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
        earliest_block // Returns optional, so if there's no transactions, it'll get the activation height
            .unwrap_or(cmp::max(
                birthday,
                self.transaction_context.config.sapling_activation_height(),
            ))
    }

    /// Get all (unspent) utxos. pending spent utxos are included
    pub async fn get_utxos(&self) -> Vec<TransparentOutput> {
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .values()
            .flat_map(|transaction| {
                transaction
                    .transparent_outputs
                    .iter()
                    .filter(|utxo| !utxo.is_spent())
            })
            .cloned()
            .collect::<Vec<TransparentOutput>>()
    }

    /// TODO: Add Doc Comment Here!
    pub async fn last_synced_hash(&self) -> String {
        self.blocks
            .read()
            .await
            .first()
            .map(|block| block.hash())
            .unwrap_or_default()
    }

    /// TODO: How do we know that 'sapling_activation_height - 1' is only returned
    /// when it should be?  When should it be?
    pub async fn last_synced_height(&self) -> u64 {
        self.blocks
            .read()
            .await
            .first()
            .map(|block| block.height)
            .unwrap_or(self.transaction_context.config.sapling_activation_height() - 1)
    }

    /// TODO: Add Doc Comment Here!
    pub fn transactions(&self) -> Arc<RwLock<TxMapAndMaybeTrees>> {
        self.transaction_context.transaction_metadata_set.clone()
    }
}

#[cfg(test)]
mod tests {
    use orchard::note_encryption::OrchardDomain;
    use sapling_crypto::note_encryption::SaplingDomain;

    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingoconfig::ZingoConfigBuilder;

    use crate::{
        mocks::{orchard_note::OrchardCryptoNoteBuilder, SaplingCryptoNoteBuilder},
        wallet::{
            notes::{
                orchard::mocks::OrchardNoteBuilder, sapling::mocks::SaplingNoteBuilder,
                transparent::mocks::TransparentOutputBuilder,
            },
            transaction_record::mocks::TransactionRecordBuilder,
            LightWallet, WalletBase,
        },
    };

    #[tokio::test]
    async fn confirmed_balance_excluding_dust() {
        let wallet = LightWallet::new(
            ZingoConfigBuilder::default().create(),
            WalletBase::FreshEntropy,
            1,
        )
        .unwrap();
        let confirmed_tx_record = TransactionRecordBuilder::default()
            .status(ConfirmationStatus::Confirmed(80.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .note(
                        SaplingCryptoNoteBuilder::default()
                            .value(sapling_crypto::value::NoteValue::from_raw(3_000))
                            .clone(),
                    )
                    .clone(),
            )
            .orchard_notes(OrchardNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(5_000))
                            .clone(),
                    )
                    .clone(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(2_000))
                            .clone(),
                    )
                    .clone(),
            )
            .build();
        let pending_tx_record = TransactionRecordBuilder::default()
            .status(ConfirmationStatus::Pending(95.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .build();
        {
            let mut tx_map = wallet
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            tx_map
                .transaction_records_by_id
                .insert_transaction_record(confirmed_tx_record);
            tx_map
                .transaction_records_by_id
                .insert_transaction_record(pending_tx_record);
        }

        assert_eq!(
            wallet
                .confirmed_balance_excluding_dust::<SaplingDomain>()
                .await,
            Some(400_000)
        );
        assert_eq!(
            wallet
                .confirmed_balance_excluding_dust::<OrchardDomain>()
                .await,
            Some(1_605_000)
        );
    }
}
