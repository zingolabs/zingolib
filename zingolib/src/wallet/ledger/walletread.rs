use zcash_client_backend::{data_api::WalletRead, keys::UnifiedFullViewingKey};

use super::ZingoLedger;
use crate::error::ZingoLibError;

impl WalletRead for ZingoLedger {
    type Error = ZingoLibError;

    fn chain_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(
        &self,
        _height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        todo!()
    }

    fn block_fully_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        todo!()
    }

    fn block_max_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        todo!()
    }

    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        todo!()
    }

    fn get_target_and_anchor_heights(
        &self,
        _min_confirmations: std::num::NonZeroU32,
    ) -> Result<
        Option<(
            zcash_primitives::consensus::BlockHeight,
            zcash_primitives::consensus::BlockHeight,
        )>,
        Self::Error,
    > {
        self.current
            .iter()
            .filter_map(|(_, tr)| tr.status.get_confirmed_height())
            .max_by(|a, b| a.cmp(&b))
            .map_or_else(|| Ok(None), |max_height| Ok(Some((max_height, max_height))))
    }

    fn get_min_unspent_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        todo!()
    }

    fn get_block_hash(
        &self,
        _block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<Option<zcash_primitives::block::BlockHash>, Self::Error> {
        todo!()
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
        todo!()
    }

    fn get_tx_height(
        &self,
        _txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        todo!()
    }

    fn get_wallet_birthday(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        todo!()
    }

    fn get_account_birthday(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<zcash_primitives::consensus::BlockHeight, Self::Error> {
        todo!()
    }

    fn get_current_address(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
        todo!()
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<
        std::collections::HashMap<zcash_primitives::zip32::AccountId, UnifiedFullViewingKey>,
        Self::Error,
    > {
        todo!()
    }

    fn get_account_for_ufvk(
        &self,
        _ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<zcash_primitives::zip32::AccountId>, Self::Error> {
        todo!()
    }

    fn get_wallet_summary(
        &self,
        _min_confirmations: u32,
    ) -> Result<Option<zcash_client_backend::data_api::WalletSummary>, Self::Error> {
        todo!()
    }

    fn get_memo(
        &self,
        _note_id: zcash_client_backend::wallet::NoteId,
    ) -> Result<Option<zcash_primitives::memo::Memo>, Self::Error> {
        todo!()
    }

    fn get_transaction(
        &self,
        _txid: zcash_primitives::transaction::TxId,
    ) -> Result<zcash_primitives::transaction::Transaction, Self::Error> {
        todo!()
    }

    fn get_sapling_nullifiers(
        &self,
        _query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<
        Vec<(
            zcash_primitives::zip32::AccountId,
            sapling_crypto::Nullifier,
        )>,
        Self::Error,
    > {
        todo!()
    }

    fn get_transparent_receivers(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<
        std::collections::HashMap<
            zcash_primitives::legacy::TransparentAddress,
            zcash_client_backend::address::AddressMetadata,
        >,
        Self::Error,
    > {
        todo!()
    }

    fn get_transparent_balances(
        &self,
        _account: zcash_primitives::zip32::AccountId,
        _max_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<
        std::collections::HashMap<
            zcash_primitives::legacy::TransparentAddress,
            zcash_primitives::transaction::components::Amount,
        >,
        Self::Error,
    > {
        todo!()
    }

    fn get_account_ids(&self) -> Result<Vec<zcash_primitives::zip32::AccountId>, Self::Error> {
        todo!()
    }

    fn get_orchard_nullifiers(
        &self,
        _query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(zcash_primitives::zip32::AccountId, orchard::note::Nullifier)>, Self::Error>
    {
        todo!()
    }
}
