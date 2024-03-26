use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::record_book::NoteRecordIdentifier;

use super::*;

impl LightClient {
    pub async fn new_client_from_save_buffer(&self) -> ZingoLibResult<Self> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(ZingoLibError::CantReadWallet)
    }
    pub async fn check_chain_matched_proposal(
        &self,
        proposal: Proposal<FeeRule, NoteRecordIdentifier>,
        txids: Vec<TxId>,
        total_balance_before: &mut u64,
    ) -> ZingoLibResult<()> {
        let tmamt = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await;

        let steps = proposal.steps();
        assert_eq!(txids.len(), steps.len());
        for (index, step) in proposal.steps().iter().enumerate() {
            let created_txid = txids[index];
            //check that the new transaction has the properties
            let created_transaction = tmamt
                .current
                .get(&created_txid)
                .expect("new txid is in record");
            assert_eq!(created_transaction.status.is_confirmed(), true);
            assert_eq!(created_transaction.is_outgoing_transaction(), true);
            let transaction_balance = created_transaction.net_spent();
            assert_eq!(transaction_balance, step.balance().total().into_u64());
            *total_balance_before = *total_balance_before - transaction_balance;

            // we could maybe check that all input transparents are spent
            // if let Some(transparent_inputs) = step.transparent_inputs() {
            //     // for note in transparent_inputs {
            //     //     //pub fn get_note
            //     //     let script ;
            //     //     let txid_funded = identifier.txid;
            //     //     let transaction_funded = tmamt
            //     //         .current
            //     //         .get(&txid_funded)
            //     //         .expect("funding txid is in record");
            //     //     match identifier.shielded_protocol {
            //     //         zcash_client_backend::ShieldedProtocol::Sapling => {
            //     //             for note in transaction_funded.sapling_notes.iter() {
            //     //                 if note.output_index().unwrap() == identifier.index {
            //     //                     assert_eq!(note.is_spent(), true);
            //     //                 }
            //     //             }
            //     //         }
            //     //         zcash_client_backend::ShieldedProtocol::Orchard => {
            //     //             for note in transaction_funded.orchard_notes.iter() {
            //     //                 if note.output_index().unwrap() == identifier.index {
            //     //                     assert_eq!(note.is_spent(), true);
            //     //                 }
            //     //             }
            //     //         }
            //     //     }
            //     // }
            // }

            //check that all input notes are spent
            if let Some(shielded_inputs) = step.shielded_inputs() {
                for note in shielded_inputs.notes() {
                    //pub fn get_note
                    let identifier = note.internal_note_id();
                    let txid_funded = identifier.txid;
                    let transaction_funded = tmamt
                        .current
                        .get(&txid_funded)
                        .expect("funding txid is in record");
                    match identifier.shielded_protocol {
                        zcash_client_backend::ShieldedProtocol::Sapling => {
                            for note in transaction_funded.sapling_notes.iter() {
                                if note.output_index().unwrap() == identifier.index {
                                    assert_eq!(note.is_spent(), true);
                                }
                            }
                        }
                        zcash_client_backend::ShieldedProtocol::Orchard => {
                            for note in transaction_funded.orchard_notes.iter() {
                                if note.output_index().unwrap() == identifier.index {
                                    assert_eq!(note.is_spent(), true);
                                }
                            }
                        }
                    }
                }
            }
        }

        // balance = tmamt.get_total_balance();
        // assexrt_eq!(x

        Ok(())
    }
}
