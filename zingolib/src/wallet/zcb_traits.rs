// FIXME: zingo2
#![allow(unused_variables)]

use std::{collections::HashMap, num::NonZeroU32, ops::Range};

use zcash_client_backend::{
    data_api::{
        scanning::ScanRange, Account, BlockMetadata, InputSource, NullifierQuery,
        TransactionDataRequest, WalletRead, WalletSummary,
    },
    wallet::{NoteId, TransparentAddressMetadata},
};
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::{
    block::BlockHash,
    consensus::BlockHeight,
    legacy::TransparentAddress,
    memo::Memo,
    transaction::{components::amount::NonNegativeAmount, Transaction, TxId},
};

use super::{error::WalletError, LightWallet};

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

impl WalletRead for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type Account = ZingoAccount;

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
        seed: &zip32::fingerprint::SeedFingerprint,
        account_id: zip32::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn validate_seed(
        &self,
        account_id: Self::AccountId,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
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
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_summary(
        &self,
        min_confirmations: u32,
    ) -> Result<Option<WalletSummary<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_block_hash(&self, block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(&self, height: BlockHeight) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn block_fully_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn get_max_height_hash(&self) -> Result<Option<(BlockHeight, BlockHash)>, Self::Error> {
        unimplemented!()
    }

    fn block_max_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn suggest_scan_ranges(&self) -> Result<Vec<ScanRange>, Self::Error> {
        unimplemented!()
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: NonZeroU32,
    ) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        unimplemented!()
    }

    fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error> {
        unimplemented!()
    }

    fn get_memo(&self, note_id: NoteId) -> Result<Option<Memo>, Self::Error> {
        unimplemented!()
    }

    fn get_transaction(&self, txid: TxId) -> Result<Option<Transaction>, Self::Error> {
        unimplemented!()
    }

    fn get_sapling_nullifiers(
        &self,
        query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_orchard_nullifiers(
        &self,
        query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_receivers(
        &self,
        _account: Self::AccountId,
    ) -> Result<HashMap<TransparentAddress, Option<TransparentAddressMetadata>>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_balances(
        &self,
        _account: Self::AccountId,
        _max_height: BlockHeight,
    ) -> Result<HashMap<TransparentAddress, NonNegativeAmount>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<TransparentAddressMetadata>, Self::Error> {
        unimplemented!()
    }

    fn get_known_ephemeral_addresses(
        &self,
        _account: Self::AccountId,
        _index_range: Option<Range<u32>>,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, Self::Error> {
        unimplemented!()
    }

    fn transaction_data_requests(&self) -> Result<Vec<TransactionDataRequest>, Self::Error> {
        unimplemented!()
    }
}

impl InputSource for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type NoteRef = NoteId;

    fn get_spendable_note(
        &self,
        txid: &TxId,
        protocol: zcash_client_backend::ShieldedProtocol,
        index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        unimplemented!()
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<zcash_client_backend::data_api::SpendableNotes<Self::NoteRef>, Self::Error> {
        unimplemented!()
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }

    fn get_spendable_transparent_outputs(
        &self,
        _address: &TransparentAddress,
        _target_height: BlockHeight,
        _min_confirmations: u32,
    ) -> Result<Vec<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }
}
