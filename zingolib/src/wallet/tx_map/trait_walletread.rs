//! in this mod, we implement an LRZ type on the TxMap

use super::{TxMap, TxMapTraitError};
use secrecy::SecretVec;
use shardtree::store::ShardStore;
use zcash_client_backend::{
    data_api::{Account, WalletRead},
    keys::UnifiedFullViewingKey,
    wallet::TransparentAddressMetadata,
};
use zcash_primitives::{
    consensus::BlockHeight,
    legacy::keys::{NonHardenedChildIndex, TransparentKeyScope},
};
use zip32::Scope;

/// This is a facade for using LRZ traits. In actuality, Zingo does not use multiple accounts in one wallet.
pub struct ZingoAccount(zip32::AccountId, UnifiedFullViewingKey);

impl Account for ZingoAccount {
    type AccountId = zip32::AccountId;

    fn id(&self) -> Self::AccountId {
        self.0
    }

    fn source(&self) -> zcash_client_backend::data_api::AccountSource {
        unimplemented!()
    }

    fn ufvk(&self) -> Option<&UnifiedFullViewingKey> {
        Some(&self.1)
    }

    fn uivk(&self) -> zcash_keys::keys::UnifiedIncomingViewingKey {
        unimplemented!()
    }
}

/// some of these functions, initially those required for calculate_transaction, will be implemented
/// every doc-comment on a trait method is copied from the trait declaration in zcash_client_backend
/// except those doc-comments starting with IMPL:
impl WalletRead for TxMap {
    type Error = TxMapTraitError;
    type AccountId = zip32::AccountId;
    type Account = ZingoAccount;

    /// Returns the account corresponding to a given [`UnifiedFullViewingKey`], if any.
    /// IMPL: partially implemented. zingo uses the default account. when we expand account functionality, this will be updated
    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        // todo we could assert that the ufvk matches, or return error.
        Ok(Some(ZingoAccount(Self::AccountId::ZERO, ufvk.clone())))
    }

    /// Returns the default target height (for the block in which a new
    /// transaction would be mined) and anchor height (to use for a new
    /// transaction), given the range of block heights that the backend
    /// knows about.
    ///
    /// This will return `Ok(None)` if no block data is present in the database.
    /// IMPL: fully implemented. the target height is always the next block after the last block fetched from the server, and the anchor is a variable depth below.
    /// IMPL: tested
    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: std::num::NonZeroU32,
    ) -> Result<
        Option<(
            zcash_primitives::consensus::BlockHeight,
            zcash_primitives::consensus::BlockHeight,
        )>,
        Self::Error,
    > {
        match self.witness_trees() {
            Some(trees) => {
                let opt_max_downloaded_height =
                    match trees.witness_tree_orchard.store().max_checkpoint_id() {
                        Ok(height) => height,
                        Err(e) => match e {}, // Infallible
                    };

                Ok(opt_max_downloaded_height
                    .map(|max_downloaded_height| max_downloaded_height + 1)
                    .map(|anticipated_next_block_height| {
                        (
                            anticipated_next_block_height,
                            BlockHeight::from_u32(std::cmp::max(
                                1,
                                u32::from(anticipated_next_block_height)
                                    .saturating_sub(u32::from(min_confirmations)),
                            )),
                        )
                    }))
            }
            None => Err(TxMapTraitError::NoSpendCapability),
        }
    }

    /// Returns the block height in which the specified transaction was mined, or `Ok(None)` if the
    /// transaction is not in the main chain.
    /// IMPL: fully implemented
    fn get_tx_height(
        &self,
        txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        Ok(self
            .transaction_records_by_id
            .get(&txid)
            .and_then(|transaction| transaction.status.get_confirmed_height()))
    }

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        Ok(vec![(Self::AccountId::ZERO)])
    }
    fn get_account(
        &self,
        _account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }
    fn get_derived_account(
        &self,
        _seed: &zip32::fingerprint::SeedFingerprint,
        _account_id: zcash_primitives::zip32::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }
    fn validate_seed(
        &self,
        _account_id: Self::AccountId,
        _seed: &SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }
    fn get_current_address(
        &self,
        _account: Self::AccountId,
    ) -> Result<Option<zcash_keys::address::UnifiedAddress>, Self::Error> {
        unimplemented!()
    }
    fn get_account_birthday(
        &self,
        _account: Self::AccountId,
    ) -> Result<zcash_primitives::consensus::BlockHeight, Self::Error> {
        unimplemented!()
    }
    fn get_wallet_birthday(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        unimplemented!()
    }
    fn get_wallet_summary(
        &self,
        _min_confirmations: u32,
    ) -> Result<Option<zcash_client_backend::data_api::WalletSummary<Self::AccountId>>, Self::Error>
    {
        unimplemented!()
    }
    fn chain_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        self.witness_trees()
            .ok_or(TxMapTraitError::NoSpendCapability)?
            .witness_tree_orchard
            .store()
            .max_checkpoint_id()
            .map_err(|e| match e {})
    }
    fn get_block_hash(
        &self,
        _block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<Option<zcash_primitives::block::BlockHash>, Self::Error> {
        unimplemented!()
    }
    fn block_metadata(
        &self,
        _height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        unimplemented!()
    }
    fn block_fully_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        unimplemented!()
    }
    fn get_max_height_hash(
        &self,
    ) -> Result<
        Option<(
            zcash_primitives::consensus::BlockHeight,
            zcash_primitives::block::BlockHash,
        )>,
        Self::Error,
    > {
        unimplemented!()
    }
    fn block_max_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        unimplemented!()
    }
    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        unimplemented!()
    }
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<std::collections::HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error>
    {
        unimplemented!()
    }
    fn get_memo(
        &self,
        _note_id: zcash_client_backend::wallet::NoteId,
    ) -> Result<Option<zcash_primitives::memo::Memo>, Self::Error> {
        unimplemented!()
    }
    fn get_transaction(
        &self,
        _txid: zcash_primitives::transaction::TxId,
    ) -> Result<std::option::Option<zcash_primitives::transaction::Transaction>, Self::Error> {
        unimplemented!()
    }
    fn get_sapling_nullifiers(
        &self,
        _query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }
    fn get_orchard_nullifiers(
        &self,
        _query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }
    fn seed_relevance_to_derived_accounts(
        &self,
        _seed: &SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    /// Our receivers are all externally scoped, which will need to be reevaluated for zip320
    fn get_transparent_receivers(
        &self,
        _account: Self::AccountId,
    ) -> Result<
        std::collections::HashMap<
            zcash_primitives::legacy::TransparentAddress,
            Option<zcash_client_backend::wallet::TransparentAddressMetadata>,
        >,
        Self::Error,
    > {
        Ok(self
            .transparent_child_addresses
            .iter()
            .map(|(index, taddr)| {
                (
                    *taddr,
                    NonHardenedChildIndex::from_index(*index as u32).map(|nhc_index| {
                        TransparentAddressMetadata::new(
                            TransparentKeyScope::from(Scope::External),
                            nhc_index,
                        )
                    }),
                )
            })
            .collect())
    }

    fn get_transparent_balances(
        &self,
        _account: Self::AccountId,
        _max_height: BlockHeight,
    ) -> Result<
        std::collections::HashMap<
            zcash_primitives::legacy::TransparentAddress,
            zcash_primitives::transaction::components::amount::NonNegativeAmount,
        >,
        Self::Error,
    > {
        Ok(std::collections::HashMap::new())
    }

    fn transaction_data_requests(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::TransactionDataRequest>, Self::Error> {
        unimplemented!()
    }

    fn get_known_ephemeral_addresses(
        &self,
        _account: Self::AccountId,
        _index_range: Option<std::ops::Range<u32>>,
    ) -> Result<
        Vec<(
            zcash_primitives::legacy::TransparentAddress,
            TransparentAddressMetadata,
        )>,
        Self::Error,
    > {
        Ok(self.rejection_addresses.iter().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::proptest;
    use std::num::NonZeroU32;

    use zcash_client_backend::data_api::WalletRead;
    use zcash_primitives::consensus::BlockHeight;
    use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;

    use crate::wallet::transaction_record::mocks::TransactionRecordBuilder;

    use super::TxMap;
    use super::TxMapTraitError;

    #[test]
    fn get_target_and_anchor_heights() {
        let mut transaction_records_and_maybe_trees = TxMap::new_with_witness_trees_address_free();
        transaction_records_and_maybe_trees
            .spending_data
            .as_mut()
            .unwrap()
            .witness_trees_mut()
            .witness_tree_orchard
            .checkpoint(8421.into())
            .unwrap();

        assert_eq!(
            transaction_records_and_maybe_trees
                .get_target_and_anchor_heights(NonZeroU32::new(10).unwrap())
                .unwrap()
                .unwrap(),
            (BlockHeight::from_u32(8422), BlockHeight::from_u32(8412))
        );
    }

    #[test]
    fn get_target_and_anchor_heights_none() {
        let transaction_records_and_maybe_trees = TxMap::new_with_witness_trees_address_free();
        assert_eq!(
            transaction_records_and_maybe_trees
                .get_target_and_anchor_heights(NonZeroU32::new(10).unwrap())
                .unwrap(),
            None
        );
    }

    #[test]
    fn get_target_and_anchor_heights_err() {
        let transaction_records_and_maybe_trees = TxMap::new_treeless_address_free();
        assert!(matches!(
            transaction_records_and_maybe_trees
                .get_target_and_anchor_heights(NonZeroU32::new(10).unwrap())
                .err()
                .unwrap(),
            TxMapTraitError::NoSpendCapability
        ));
    }

    proptest! {
        #[test]
        fn get_tx_height(tx_height: u32) {
            let mut transaction_records_and_maybe_trees = TxMap::new_with_witness_trees_address_free();

            let transaction_record = TransactionRecordBuilder::default().randomize_txid().status(Confirmed(tx_height.into()))
            .build();

            let txid = transaction_record.txid;
            // these first three outputs will not trigger min_unspent_note
            transaction_records_and_maybe_trees
                .transaction_records_by_id
                .insert_transaction_record(
                    transaction_record
                );

            assert_eq!(transaction_records_and_maybe_trees.get_tx_height(txid).unwrap().unwrap(), BlockHeight::from_u32(tx_height));
        }
    }
}
