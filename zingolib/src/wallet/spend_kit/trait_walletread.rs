use secrecy::SecretVec;
use shardtree::store::ShardStore;
use zcash_client_backend::data_api::{Account, WalletRead};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{consensus::BlockHeight, zip32::AccountId};

use crate::error::ZingoLibError;

use super::SpendKit;

pub struct ZingoAccount(AccountId, UnifiedFullViewingKey);

impl Account<AccountId> for ZingoAccount {
    fn id(&self) -> AccountId {
        self.0
    }

    fn source(&self) -> zcash_client_backend::data_api::AccountSource {
        todo!()
    }

    fn ufvk(&self) -> Option<&UnifiedFullViewingKey> {
        Some(&self.1)
    }

    fn uivk(&self) -> zcash_keys::keys::UnifiedIncomingViewingKey {
        todo!()
    }
}

impl WalletRead for SpendKit<'_, '_> {
    type Error = ZingoLibError;
    type AccountId = AccountId;
    type Account = ZingoAccount;

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

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        Ok(Some(ZingoAccount(AccountId::ZERO, ufvk.clone())))
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
        let highest_block_height = match self.trees.witness_tree_orchard.store().max_checkpoint_id()
        {
            Ok(height) => height,
            // Infallible
            Err(e) => match e {},
        };

        let target_height = highest_block_height.map(|height| height + 1);

        Ok(target_height.map(|height| {
            (
                height,
                BlockHeight::from_u32(std::cmp::max(
                    1,
                    u32::from(height).saturating_sub(u32::from(min_confirmations)),
                )),
            )
        }))
    }

    fn get_min_unspent_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        Ok(self
            .record_map
            .map
            .values()
            .fold(None, |height, transaction| {
                let transaction_height = transaction.status.get_confirmed_height();
                match (height, transaction_height) {
                    (None, None) => None,
                    (Some(h), None) | (None, Some(h)) => Some(h),
                    (Some(h1), Some(h2)) => Some(std::cmp::min(h1, h2)),
                }
            }))
    }

    fn get_tx_height(
        &self,
        txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        Ok(self
            .record_map
            .map
            .get(&txid)
            .and_then(|transaction| transaction.status.get_confirmed_height()))
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
        seed: &SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::{cmp::max, num::NonZeroU32, sync::Arc};

    use zingoconfig::{ChainType, ZingoConfig};

    use crate::wallet::{
        data::WitnessTrees, keys::unified::WalletCapability, transactions::TransactionRecordMap,
    };

    use super::*;

    #[test]
    fn target_anchor_heights() {
        for tree_height in 1..=10 {
            println!("testing tree height {tree_height}");
            let params = ChainType::Mainnet;
            let record_map = &TransactionRecordMap::new_empty();
            let tree_height = BlockHeight::from_u32(tree_height);
            let trees = &mut WitnessTrees::default();
            let latest_proposal = &mut None;
            let local_sending_transactions = Vec::new();
            trees.add_checkpoint(tree_height);

            let kit = SpendKit {
                spend_cap: Arc::new(WalletCapability::new_from_seed(
                    &ZingoConfig::build(params).create(),
                    &[0; 64],
                    0,
                )),
                params,
                record_map,
                trees,
                latest_proposal,
                local_sending_transactions,
            };

            let (targ_height, anc_height) = kit
                .get_target_and_anchor_heights(NonZeroU32::new(4).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(targ_height, tree_height + 1);
            assert_eq!(
                anc_height,
                max(BlockHeight::from_u32(1), tree_height.saturating_sub(3))
            )
        }
    }
}
