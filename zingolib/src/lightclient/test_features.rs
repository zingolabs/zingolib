use crate::error::ZingoLibError;
use crate::wallet::Pool;
use nonempty::NonEmpty;
use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;

use super::*;

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn new_client_from_save_buffer(&self) -> Result<Self, ZingoLibError> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(ZingoLibError::CantReadWallet)
    }

    /// Test only lightclient method for calling `do_send` with primitive rust types
    ///
    /// # Panics
    ///
    /// Panics if the address, amount or memo conversion fails.
    /// ignores secondary
    pub async fn do_send_test_only(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        self.do_quick_send(
            self.raw_to_transaction_request(
                address_amount_memo_tuples
                    .into_iter()
                    .map(|(address, amount, memo)| {
                        (
                            address.to_string(),
                            amount as u32,
                            memo.map(|memo| memo.to_string()),
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        )
        .await
        .map(|txid| txid.first().to_string())
    }

    /// Test only lightclient method for calling `do_shield` with an address as &str
    ///
    /// # Panics
    ///
    /// Panics if the address conversion fails.
    #[cfg(feature = "test-features")]
    pub async fn do_shield_test_only(
        &self,
        _pools_to_shield: &[Pool],
        _address: Option<&str>,
    ) -> Result<String, String> {
        todo!()
        // let address = address.map(|addr| {
        //     address_from_str(addr, &self.config().chain).expect("should be a valid address")
        // });
        // self.do_shield(pools_to_shield, address)
        //     .await
        //     .map(|txid| txid.to_string())
    }

    /// compares a proposal with a final transaction
    pub async fn check_chain_matches_proposal<T, U>(
        &self,
        proposal: Proposal<T, U>,
        txids: NonEmpty<TxId>,
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
        for (index, _step) in proposal.steps().iter().enumerate() {
            let created_txid = dbg!(txids[index]);
            //check that the new transaction has the properties
            let created_transaction = tmamt
                .transaction_records_by_id
                .get(&created_txid)
                .expect("new txid is in record");
            assert_eq!(created_transaction.status.is_confirmed(), confirmed);
            assert!(created_transaction.is_outgoing_transaction());
            let _transaction_balance = dbg!(created_transaction.net_spent());
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
