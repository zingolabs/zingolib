use secrecy::SecretVec;
use zcash_client_backend::data_api::WalletWrite;
use zcash_client_backend::keys::UnifiedSpendingKey;

use super::ZingoLedger;

impl WalletWrite for ZingoLedger {
    type UtxoRef = ();

    fn create_account(
        &mut self,
        seed: &SecretVec<u8>,
        birthday: zcash_client_backend::data_api::AccountBirthday,
    ) -> Result<(zcash_primitives::zip32::AccountId, UnifiedSpendingKey), Self::Error> {
        todo!()
    }

    fn get_next_available_address(
        &mut self,
        account: zcash_primitives::zip32::AccountId,
        request: zcash_client_backend::keys::UnifiedAddressRequest,
    ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
        todo!()
    }

    fn put_blocks(
        &mut self,
        blocks: Vec<
            zcash_client_backend::data_api::ScannedBlock<
                sapling_crypto::Nullifier,
                orchard::keys::Scope,
            >,
        >,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn update_chain_tip(
        &mut self,
        tip_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn store_decrypted_tx(
        &mut self,
        received_tx: zcash_client_backend::data_api::DecryptedTransaction,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn store_sent_tx(
        &mut self,
        sent_tx: &zcash_client_backend::data_api::SentTransaction,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn truncate_to_height(
        &mut self,
        block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        output: &zcash_client_backend::wallet::WalletTransparentOutput,
    ) -> Result<Self::UtxoRef, Self::Error> {
        todo!()
    }
}
