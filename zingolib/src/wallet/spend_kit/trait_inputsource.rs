// use zcash_client_backend::data_api::InputSource;

// use super::SpendKit;

// impl InputSource for SpendKit {
//     type Error;

//     type AccountId;

//     type NoteRef;

//     fn get_spendable_note(
//         &self,
//         txid: &zcash_primitives::transaction::TxId,
//         protocol: zcash_client_backend::ShieldedProtocol,
//         index: u32,
//     ) -> Result<
//         Option<
//             zcash_client_backend::wallet::ReceivedNote<
//                 Self::NoteRef,
//                 zcash_client_backend::wallet::Note,
//             >,
//         >,
//         Self::Error,
//     > {
//         todo!()
//     }

//     fn select_spendable_notes(
//         &self,
//         account: Self::AccountId,
//         target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
//         sources: &[zcash_client_backend::ShieldedProtocol],
//         anchor_height: zcash_primitives::consensus::BlockHeight,
//         exclude: &[Self::NoteRef],
//     ) -> Result<
//         Vec<
//             zcash_client_backend::wallet::ReceivedNote<
//                 Self::NoteRef,
//                 zcash_client_backend::wallet::Note,
//             >,
//         >,
//         Self::Error,
//     > {
//         todo!()
//     }
// }
