//! currently only implementing one method of WalletWrite

use zcash_client_backend::data_api::WalletWrite;

use crate::wallet::tx_map::TxMapTraitError;

use super::TxMap;

impl WalletWrite for TxMap {
    type UtxoRef = u32;

    fn create_account(
        &mut self,
        seed: &secrecy::SecretVec<u8>,
        birthday: &zcash_client_backend::data_api::AccountBirthday,
    ) -> Result<(Self::AccountId, zcash_keys::keys::UnifiedSpendingKey), Self::Error> {
        unimplemented!()
    }

    fn get_next_available_address(
        &mut self,
        account: Self::AccountId,
        request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<zcash_keys::address::UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn update_chain_tip(
        &mut self,
        tip_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_blocks(
        &mut self,
        from_state: &zcash_client_backend::data_api::chain::ChainState,
        blocks: Vec<zcash_client_backend::data_api::ScannedBlock<Self::AccountId>>,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        output: &zcash_client_backend::wallet::WalletTransparentOutput,
    ) -> Result<Self::UtxoRef, Self::Error> {
        unimplemented!()
    }

    fn store_decrypted_tx(
        &mut self,
        received_tx: zcash_client_backend::data_api::DecryptedTransaction<Self::AccountId>,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn store_sent_tx(
        &mut self,
        sent_tx: &zcash_client_backend::data_api::SentTransaction<Self::AccountId>,
    ) -> Result<(), Self::Error> {
        // match self.spending_data {
        //     None => Err(TxMapTraitError::NoSpendCapability),
        //     Some(ref mut spending_data) => {
        //         spending_data
        //             .cached_transactions_mut()
        //             .insert(sent_tx.tx().txid(), *sent_tx.tx().clone());
        //         Ok(())
        //     }
        // }
        unimplemented!()
    }

    fn truncate_to_height(
        &mut self,
        block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }
}
