// use zcash_client_backend::data_api::WalletWrite;

// use super::SpendKit;

// impl WalletWrite for SpendKit {
//     type UtxoRef;

//     fn create_account(
//         &mut self,
//         seed: &SecretVec<u8>,
//         birthday: zcash_client_backend::data_api::AccountBirthday,
//     ) -> Result<(Self::AccountId, UnifiedSpendingKey), Self::Error> {
//         todo!()
//     }

//     fn get_next_available_address(
//         &mut self,
//         account: Self::AccountId,
//         request: zcash_keys::keys::UnifiedAddressRequest,
//     ) -> Result<Option<zcash_keys::address::UnifiedAddress>, Self::Error> {
//         todo!()
//     }

//     fn update_chain_tip(
//         &mut self,
//         tip_height: zcash_primitives::consensus::BlockHeight,
//     ) -> Result<(), Self::Error> {
//         todo!()
//     }

//     fn put_blocks(
//         &mut self,
//         from_state: &zcash_client_backend::data_api::chain::ChainState,
//         blocks: Vec<zcash_client_backend::data_api::ScannedBlock<Self::AccountId>>,
//     ) -> Result<(), Self::Error> {
//         todo!()
//     }

//     fn put_received_transparent_utxo(
//         &mut self,
//         output: &zcash_client_backend::wallet::WalletTransparentOutput,
//     ) -> Result<Self::UtxoRef, Self::Error> {
//         todo!()
//     }

//     fn store_decrypted_tx(
//         &mut self,
//         received_tx: zcash_client_backend::data_api::DecryptedTransaction<Self::AccountId>,
//     ) -> Result<(), Self::Error> {
//         todo!()
//     }

//     fn store_sent_tx(
//         &mut self,
//         sent_tx: &zcash_client_backend::data_api::SentTransaction<Self::AccountId>,
//     ) -> Result<(), Self::Error> {
//         todo!()
//     }

//     fn truncate_to_height(
//         &mut self,
//         block_height: zcash_primitives::consensus::BlockHeight,
//     ) -> Result<(), Self::Error> {
//         todo!()
//     }
// }
