//! currently only implementing one method of WalletWrite

use std::iter;

use zcash_client_backend::data_api::WalletWrite;

use super::{TxMap, TxMapTraitError};

impl WalletWrite for TxMap {
    type UtxoRef = u32;

    fn create_account(
        &mut self,
        _seed: &secrecy::SecretVec<u8>,
        _birthday: &zcash_client_backend::data_api::AccountBirthday,
    ) -> Result<(Self::AccountId, zcash_keys::keys::UnifiedSpendingKey), Self::Error> {
        unimplemented!()
    }

    fn get_next_available_address(
        &mut self,
        _account: Self::AccountId,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<zcash_keys::address::UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn update_chain_tip(
        &mut self,
        _tip_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_blocks(
        &mut self,
        _from_state: &zcash_client_backend::data_api::chain::ChainState,
        _blocks: Vec<zcash_client_backend::data_api::ScannedBlock<Self::AccountId>>,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        _output: &zcash_client_backend::wallet::WalletTransparentOutput,
    ) -> Result<Self::UtxoRef, Self::Error> {
        unimplemented!()
    }

    fn store_decrypted_tx(
        &mut self,
        _received_tx: zcash_client_backend::data_api::DecryptedTransaction<Self::AccountId>,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn store_transactions_to_be_sent(
        &mut self,
        transactions: &[zcash_client_backend::data_api::SentTransaction<Self::AccountId>],
    ) -> Result<(), Self::Error> {
        for tx in transactions {
            let tx = tx.tx();
            let mut raw_tx = vec![];
            tx.write(&mut raw_tx)
                .map_err(TxMapTraitError::TransactionWrite)?;

            if let Some(spending_data) = self.spending_data_mut() {
                spending_data
                    .cached_raw_transactions_mut()
                    .push((tx.txid(), raw_tx));
            } else {
                return Err(TxMapTraitError::NoSpendCapability);
            }
        }
        Ok(())
    }

    fn truncate_to_height(
        &mut self,
        _block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<zcash_primitives::consensus::BlockHeight, TxMapTraitError> {
        unimplemented!()
    }

    fn import_account_hd(
        &mut self,
        _seed: &secrecy::SecretVec<u8>,
        _account_index: zip32::AccountId,
        _birthday: &zcash_client_backend::data_api::AccountBirthday,
    ) -> Result<(Self::Account, zcash_keys::keys::UnifiedSpendingKey), Self::Error> {
        unimplemented!()
    }

    fn import_account_ufvk(
        &mut self,
        _unified_key: &zcash_keys::keys::UnifiedFullViewingKey,
        _birthday: &zcash_client_backend::data_api::AccountBirthday,
        _purpose: zcash_client_backend::data_api::AccountPurpose,
    ) -> Result<Self::Account, Self::Error> {
        unimplemented!()
    }

    fn set_transaction_status(
        &mut self,
        _txid: zcash_primitives::transaction::TxId,
        _status: zcash_client_backend::data_api::TransactionStatus,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn reserve_next_n_ephemeral_addresses(
        &mut self,
        _account_id: Self::AccountId,
        n: usize,
    ) -> Result<
        Vec<(
            zcash_primitives::legacy::TransparentAddress,
            zcash_client_backend::wallet::TransparentAddressMetadata,
        )>,
        Self::Error,
    > {
        self.spending_data()
            .as_ref()
            .map(|spending_data| {
                iter::repeat_with(|| {
                    crate::wallet::data::new_rejection_address(
                        &self.rejection_addresses,
                        spending_data.rejection_ivk(),
                    )
                    .map_err(TxMapTraitError::TexSendError)
                })
                .take(n)
                .collect::<Result<_, _>>()
            })
            .transpose()?
            .ok_or(TxMapTraitError::NoSpendCapability)
    }
}
