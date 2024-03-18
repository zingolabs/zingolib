use secrecy::SecretVec;
use zcash_client_backend::{data_api::WalletRead, keys::UnifiedFullViewingKey};
use zcash_primitives::zip32::AccountId;

use crate::error::ZingoLibError;

use super::LightWallet;

impl WalletRead for LightWallet {
    type Error = ZingoLibError;
    type AccountId = AccountId;

    type Account = (Self::AccountId, UnifiedFullViewingKey);

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_account(
        &self,
        account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_derived_account(
        &self,
        seed: &zcash_client_backend::keys::HdSeedFingerprint,
        account_id: zcash_primitives::zip32::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn validate_seed(
        &self,
        account_id: Self::AccountId,
        seed: &SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_current_address(
        &self,
        account: Self::AccountId,
    ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(
        &self,
        account: Self::AccountId,
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
        min_confirmations: u32,
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
        block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<Option<zcash_primitives::block::BlockHash>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(
        &self,
        height: zcash_primitives::consensus::BlockHeight,
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
        unimplemented!()
    }

    fn get_min_unspent_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_tx_height(
        &self,
        txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
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
        note_id: zcash_client_backend::wallet::NoteId,
    ) -> Result<Option<zcash_primitives::memo::Memo>, Self::Error> {
        unimplemented!()
    }

    fn get_transaction(
        &self,
        txid: zcash_primitives::transaction::TxId,
    ) -> Result<zcash_primitives::transaction::Transaction, Self::Error> {
        unimplemented!()
    }

    fn get_sapling_nullifiers(
        &self,
        query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }
}
