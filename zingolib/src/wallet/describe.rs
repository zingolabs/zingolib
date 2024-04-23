//! TODO: Add Mod Discription Here!
use orchard::note_encryption::OrchardDomain;

use sapling_crypto::note_encryption::SaplingDomain;

use std::{cmp, sync::Arc};
use tokio::sync::RwLock;
use zcash_primitives::zip339::Mnemonic;

use zcash_note_encryption::Domain;

use zcash_primitives::consensus::BlockHeight;

use crate::wallet::data::TransactionRecord;
use crate::wallet::notes::OutputInterface;
use crate::wallet::notes::ShieldedNoteInterface;

use crate::wallet::traits::Diversifiable as _;

use super::keys::unified::{Capability, WalletCapability};
use super::notes::TransparentOutput;
use super::traits::DomainWalletExt;
use super::traits::Recipient;

use super::{data::BlockData, tx_map_and_maybe_trees::TxMapAndMaybeTrees};

use super::LightWallet;
impl LightWallet {
    /// TODO: Add Doc Comment Here!
    #[allow(clippy::type_complexity)]
    pub async fn shielded_balance<D>(
        &self,
        target_addr: Option<String>,
        filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool + '_>],
    ) -> Option<u64>
    where
        D: DomainWalletExt,
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        let fvk = D::wc_to_fvk(&self.wallet_capability()).ok()?;
        let filter_notes_by_target_addr = |notedata: &&D::WalletNote| match target_addr.as_ref() {
            Some(addr) => {
                let diversified_address =
                    &fvk.diversified_address(*notedata.diversifier()).unwrap();
                *addr
                    == diversified_address
                        .b32encode_for_network(&self.transaction_context.config.chain)
            }
            None => true, // If the addr is none, then get all addrs.
        };
        Some(
            self.transaction_context
                .transaction_metadata_set
                .read()
                .await
                .transaction_records_by_id
                .values()
                .map(|transaction| {
                    let mut filtered_notes: Box<dyn Iterator<Item = &D::WalletNote>> = Box::new(
                        D::WalletNote::transaction_metadata_notes(transaction)
                            .iter()
                            .filter(filter_notes_by_target_addr),
                    );
                    // All filters in iterator are applied, by this loop
                    for filtering_fn in filters {
                        filtered_notes =
                            Box::new(filtered_notes.filter(|nnmd| filtering_fn(nnmd, transaction)))
                    }
                    filtered_notes
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
    pub async fn spendable_orchard_balance(&self, target_addr: Option<String>) -> Option<u64> {
        if let Capability::Spend(_) = self.wallet_capability().orchard {
            self.verified_balance::<OrchardDomain>(target_addr).await
        } else {
            None
        }
    }

    /// TODO: Add Doc Comment Here!
    pub async fn spendable_sapling_balance(&self, target_addr: Option<String>) -> Option<u64> {
        if let Capability::Spend(_) = self.wallet_capability().sapling {
            self.verified_balance::<SaplingDomain>(target_addr).await
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
    pub async fn unverified_balance<D: DomainWalletExt>(
        &self,
        target_addr: Option<String>,
    ) -> Option<u64>
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
        self.shielded_balance::<D>(target_addr, filters).await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn verified_balance<D: DomainWalletExt>(
        &self,
        target_addr: Option<String>,
    ) -> Option<u64>
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
        self.shielded_balance::<D>(target_addr, filters).await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn maybe_verified_orchard_balance(&self, addr: Option<String>) -> Option<u64> {
        self.shielded_balance::<OrchardDomain>(addr, &[]).await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn maybe_verified_sapling_balance(&self, addr: Option<String>) -> Option<u64> {
        self.shielded_balance::<SaplingDomain>(addr, &[]).await
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
            Some((height, anchor_offset)) => height - anchor_offset as u32 - 1,
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

    /// Get all (unspent) utxos. Unconfirmed spent utxos are included
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
