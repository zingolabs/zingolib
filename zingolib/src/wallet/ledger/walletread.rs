use zcash_client_backend::{data_api::WalletRead, keys::UnifiedFullViewingKey};
use zcash_primitives::zip32::AccountId;

use super::TxMapAndMaybeTrees;
use crate::error::ZingoLibError;

impl WalletRead for TxMapAndMaybeTrees {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;

    fn chain_height(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
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
        _min_confirmations: std::num::NonZeroU32,
    ) -> Result<
        Option<(
            zcash_primitives::consensus::BlockHeight,
            zcash_primitives::consensus::BlockHeight,
        )>,
        Self::Error,
    > {
        // review!: in previous algorithms, we subtract MAX_REORG_HEIGHT from the second value. Why?
        Ok(self.highest_known_block_height().map(|h| (h, h)))
    }

    fn get_min_unspent_height(
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

    fn get_tx_height(
        &self,
        _txid: zcash_primitives::transaction::TxId,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_birthday(
        &self,
    ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<zcash_primitives::consensus::BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn get_current_address(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<
        std::collections::HashMap<zcash_primitives::zip32::AccountId, UnifiedFullViewingKey>,
        Self::Error,
    > {
        unimplemented!()
    }

    fn get_account_for_ufvk(
        &self,
        _ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<zcash_primitives::zip32::AccountId>, Self::Error> {
        // review! why is this function called in spend?
        Ok(Some(AccountId::ZERO))
    }

    fn get_wallet_summary(
        &self,
        _min_confirmations: u32,
    ) -> Result<Option<zcash_client_backend::data_api::WalletSummary<Self::AccountId>>, Self::Error>
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
    ) -> Result<zcash_primitives::transaction::Transaction, Self::Error> {
        unimplemented!()
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
        unimplemented!()
    }

    fn get_transparent_receivers(
        &self,
        _account: zcash_primitives::zip32::AccountId,
    ) -> Result<
        std::collections::HashMap<
            zcash_primitives::legacy::TransparentAddress,
            Option<zcash_client_backend::wallet::TransparentAddressMetadata>,
        >,
        Self::Error,
    > {
        unimplemented!()
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
        unimplemented!()
    }

    fn get_account_ids(&self) -> Result<Vec<zcash_primitives::zip32::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_orchard_nullifiers(
        &self,
        _query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(zcash_primitives::zip32::AccountId, orchard::note::Nullifier)>, Self::Error>
    {
        unimplemented!()
    }
}

// impl WalletRead for &mut ZingoLedger {
//     type Error = ZingoLibError;
//     type AccountId = zcash_primitives::zip32::AccountId;

//     fn chain_height(
//         &self,
//     ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
//         self.chain_height()
//     }

//     fn block_metadata(
//         &self,
//         height: zcash_primitives::consensus::BlockHeight,
//     ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
//         self.block_metadata(height)
//     }

//     fn block_fully_scanned(
//         &self,
//     ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
//         self.block_fully_scanned()
//     }

//     fn block_max_scanned(
//         &self,
//     ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
//         self.block_max_scanned()
//     }

//     fn suggest_scan_ranges(
//         &self,
//     ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
//         self.suggest_scan_ranges()
//     }

//     fn get_target_and_anchor_heights(
//         &self,
//         min_confirmations: std::num::NonZeroU32,
//     ) -> Result<
//         Option<(
//             zcash_primitives::consensus::BlockHeight,
//             zcash_primitives::consensus::BlockHeight,
//         )>,
//         Self::Error,
//     > {
//         self.get_target_and_anchor_heights(min_confirmations)
//     }

//     fn get_min_unspent_height(
//         &self,
//     ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
//         self.get_min_unspent_height()
//     }

//     fn get_block_hash(
//         &self,
//         block_height: zcash_primitives::consensus::BlockHeight,
//     ) -> Result<Option<zcash_primitives::block::BlockHash>, Self::Error> {
//         self.get_block_hash(block_height)
//     }

//     fn get_max_height_hash(
//         &self,
//     ) -> Result<
//         Option<(
//             zcash_primitives::consensus::BlockHeight,
//             zcash_primitives::block::BlockHash,
//         )>,
//         Self::Error,
//     > {
//         self.get_max_height_hash()
//     }

//     fn get_tx_height(
//         &self,
//         txid: zcash_primitives::transaction::TxId,
//     ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
//         self.get_tx_height(txid)
//     }

//     fn get_wallet_birthday(
//         &self,
//     ) -> Result<Option<zcash_primitives::consensus::BlockHeight>, Self::Error> {
//         self.get_wallet_birthday()
//     }

//     fn get_account_birthday(
//         &self,
//         account: zcash_primitives::zip32::AccountId,
//     ) -> Result<zcash_primitives::consensus::BlockHeight, Self::Error> {
//         self.get_account_birthday(account)
//     }

//     fn get_current_address(
//         &self,
//         account: zcash_primitives::zip32::AccountId,
//     ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
//         self.get_current_address(account)
//     }

//     fn get_unified_full_viewing_keys(
//         &self,
//     ) -> Result<
//         std::collections::HashMap<zcash_primitives::zip32::AccountId, UnifiedFullViewingKey>,
//         Self::Error,
//     > {
//         self.get_unified_full_viewing_keys()
//     }

//     fn get_account_for_ufvk(
//         &self,
//         ufvk: &UnifiedFullViewingKey,
//     ) -> Result<Option<zcash_primitives::zip32::AccountId>, Self::Error> {
//         self.get_account_for_ufvk(ufvk)
//     }

//     fn get_wallet_summary(
//         &self,
//         min_confirmations: u32,
//     ) -> Result<Option<zcash_client_backend::data_api::WalletSummary<Self::AccountId>>, Self::Error>
//     {
//         self.get_wallet_summary(min_confirmations)
//     }

//     fn get_memo(
//         &self,
//         _note_id: zcash_client_backend::wallet::NoteId,
//     ) -> Result<Option<zcash_primitives::memo::Memo>, Self::Error> {
//         unimplemented!()
//     }

//     fn get_transaction(
//         &self,
//         _txid: zcash_primitives::transaction::TxId,
//     ) -> Result<zcash_primitives::transaction::Transaction, Self::Error> {
//         unimplemented!()
//     }

//     fn get_sapling_nullifiers(
//         &self,
//         _query: zcash_client_backend::data_api::NullifierQuery,
//     ) -> Result<
//         Vec<(
//             zcash_primitives::zip32::AccountId,
//             sapling_crypto::Nullifier,
//         )>,
//         Self::Error,
//     > {
//         unimplemented!()
//     }

//     fn get_transparent_receivers(
//         &self,
//         _account: zcash_primitives::zip32::AccountId,
//     ) -> Result<
//         std::collections::HashMap<
//             zcash_primitives::legacy::TransparentAddress,
//             Option<zcash_client_backend::wallet::TransparentAddressMetadata>,
//         >,
//         Self::Error,
//     > {
//         unimplemented!()
//     }

//     fn get_transparent_balances(
//         &self,
//         _account: zcash_primitives::zip32::AccountId,
//         _max_height: zcash_primitives::consensus::BlockHeight,
//     ) -> Result<
//         std::collections::HashMap<
//             zcash_primitives::legacy::TransparentAddress,
//             zcash_primitives::transaction::components::Amount,
//         >,
//         Self::Error,
//     > {
//         unimplemented!()
//     }

//     fn get_account_ids(&self) -> Result<Vec<zcash_primitives::zip32::AccountId>, Self::Error> {
//         unimplemented!()
//     }

//     fn get_orchard_nullifiers(
//         &self,
//         _query: zcash_client_backend::data_api::NullifierQuery,
//     ) -> Result<Vec<(zcash_primitives::zip32::AccountId, orchard::note::Nullifier)>, Self::Error>
//     {
//         unimplemented!()
//     }
// }
