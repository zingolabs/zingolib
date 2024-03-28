use zcash_client_backend::proposal::{Proposal, Step};
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::{record_book::NoteRecordIdentifier, transactions::Proposa};

use super::*;

pub fn step_net_spent(
    step: &Step<NoteRecordIdentifier>,
) -> Result<u64, zcash_primitives::transaction::components::amount::BalanceError> {
    let fee = step.balance().fee_required().into_u64();
    let sent = step.transaction_request().total()?;
    Ok(fee + sent.into_u64())
}

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
    pub async fn check_chain_matches_proposal(
        &self,
        proposal: Proposa,
        txids: Vec<TxId>,
        confirmed: bool,
        // total_balance_before: &mut u64,
    ) {
        // we will be using dbg! for now instead of assert to gain more info
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
            assert_eq!(created_transaction.status.is_confirmed(), confirmed);
            assert!(created_transaction.is_outgoing_transaction());
            let transaction_balance = created_transaction.net_spent();
            // assert_eq!(transaction_balance, step_net_spent(&step).unwrap());
            // *total_balance_before -= transaction_balance;

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
            // if let Some(shielded_inputs) = step.shielded_inputs() {
            //     for note in shielded_inputs.notes() {
            //         //pub fn get_note
            //         let identifier = note.internal_note_id();
            //         let txid_funded = identifier.txid;
            //         let transaction_funded = tmamt
            //             .current
            //             .get(&txid_funded)
            //             .expect("funding txid is in record");
            //         match identifier.shielded_protocol {
            //             zcash_client_backend::ShieldedProtocol::Sapling => {
            //                 for note in transaction_funded.sapling_notes.iter() {
            //                     if note.output_index().unwrap() == identifier.index {
            //                         assert!(note.is_spent());
            //                     }
            //                 }
            //             }
            //             zcash_client_backend::ShieldedProtocol::Orchard => {
            //                 for note in transaction_funded.orchard_notes.iter() {
            //                     if note.output_index().unwrap() == identifier.index {
            //                         assert!(note.is_spent());
            //                     }
            //                 }
            //             }
            //         }
            //     }
            // }
        }

        // balance = tmamt.get_total_balance();
        // assexrt_eq!(x
    }
}
