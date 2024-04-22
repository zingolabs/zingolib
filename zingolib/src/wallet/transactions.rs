//! Functionality for managing transactions

use secrecy::SecretVec;
use shardtree::store::ShardStore;
use zcash_client_backend::{
    data_api::{Account, WalletRead},
    keys::UnifiedFullViewingKey,
};
use zcash_primitives::consensus::BlockHeight;
use zip32::AccountId;

use crate::{
    error::ZingoLibError,
    wallet::{data::WitnessTrees, transaction_records_by_id::TransactionRecordsById},
};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub transaction_records_by_id: TransactionRecordsById,
    witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_with_witness_trees() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: None,
        }
    }
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.witness_trees.as_ref()
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.witness_trees.as_mut()
    }
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}

pub struct ZingoAccount(AccountId, UnifiedFullViewingKey);

impl Account<AccountId> for ZingoAccount {
    fn id(&self) -> AccountId {
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
impl WalletRead for TxMapAndMaybeTrees {
    type Error = ZingoLibError;
    type AccountId = AccountId;
    type Account = ZingoAccount;

    /// partially implemented. zingo uses the default account. when we expand account functionality, this will be updated
    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        Ok(Some(ZingoAccount(AccountId::ZERO, ufvk.clone())))
    }

    /// fully implemented. the target height is always the next block, and the anchor is a variable depth below.
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
        match self.witness_trees.as_ref() {
            Some(trees) => {
                let highest_block_height =
                    match trees.witness_tree_orchard.store().max_checkpoint_id() {
                        Ok(height) => height,
                        // Infallible
                        Err(e) => match e {},
                    };

                Ok(highest_block_height.map(|height| {
                    (
                        height + 1,
                        BlockHeight::from_u32(std::cmp::max(
                            1,
                            u32::from(height).saturating_sub(u32::from(min_confirmations)),
                        )),
                    )
                }))
            }
            None => Err(ZingoLibError::UnknownError),
        }
    }

    fn get_min_unspent_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        todo!()
        // Ok(self
        //     .record_map
        //     .map
        //     .values()
        //     .fold(None, |height, transaction| {
        //         let transaction_height = transaction.status.get_confirmed_height();
        //         match (height, transaction_height) {
        //             (None, None) => None,
        //             (Some(h), None) | (None, Some(h)) => Some(h),
        //             (Some(h1), Some(h2)) => Some(std::cmp::min(h1, h2)),
        //         }
        //     }))
    }

    fn get_tx_height(
        &self,
        _txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        todo!()
        // Ok(self
        //     .record_map
        //     .map
        //     .get(&txid)
        //     .and_then(|transaction| transaction.status.get_confirmed_height()))
    }

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        unimplemented!()
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
        unimplemented!()
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
    ) -> Result<std::option::Option<zcash_primitives::transaction::Transaction>, ZingoLibError>
    {
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
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use zcash_client_backend::data_api::WalletRead;
    use zcash_primitives::consensus::BlockHeight;

    #[test]
    fn test_get_target_and_anchor_heights() {
        use super::TxMapAndMaybeTrees;

        let mut transaction_records_and_maybe_trees = TxMapAndMaybeTrees::new_with_witness_trees();
        transaction_records_and_maybe_trees
            .witness_trees
            .as_mut()
            .unwrap()
            .add_checkpoint(8421.into());

        assert_eq!(
            transaction_records_and_maybe_trees
                .get_target_and_anchor_heights(NonZeroU32::new(10).unwrap())
                .unwrap()
                .unwrap(),
            (BlockHeight::from_u32(8422), BlockHeight::from_u32(8411))
        );
    }
}
