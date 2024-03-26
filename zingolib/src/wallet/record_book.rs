use std::{collections::HashMap, ops::Deref};

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::transaction::{Transaction, TxId};

use crate::error::{ZingoLibError, ZingoLibResult};

use super::transaction_record::TransactionRecord;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteRecordIdentifier {
    pub txid: TxId,
    pub shielded_protocol: ShieldedProtocol,
    pub index: u32,
}

impl std::fmt::Display for NoteRecordIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "txid {}, {:?}, index {}",
            self.txid, self.shielded_protocol, self.index,
        )
    }
}

pub struct TransparentRecordRef {
    txid: TxId,
    index: u32,
}

pub struct RefRecordBook<'a> {
    remote_transactions: &'a HashMap<TxId, TransactionRecord>,
    // review! how do we actually recognize this as canon when selecting?
    local_sending_transactions: &'a mut Vec<Vec<u8>>,
}

impl<'a> RefRecordBook<'a> {
    #[cfg(test)]
    pub fn new_empty() -> Self {
        let empty_map: HashMap<TxId, TransactionRecord> = HashMap::new();
        let empty_map_ref = Box::leak(Box::new(empty_map)); // Leak the empty hashmap to ensure its lifetime
        let mut empty_cell: Vec<Vec<u8>> = Vec::new();
        let empty_cell_ref = Box::leak(Box::new(empty_cell)); // Leak the empty hashmap to ensure its lifetime
        Self {
            remote_transactions: empty_map_ref,
            local_sending_transactions: empty_cell_ref,
        }
    }
    pub fn new_from_remote_txid_hashmap_and_spend_cell<'b>(
        remote_transactions: &'b HashMap<TxId, TransactionRecord>,
        local_sending_transactions: &'b mut Vec<Vec<u8>>,
    ) -> Self
    where
        'b: 'a, // Ensure 'b outlives 'a
    {
        Self {
            remote_transactions,
            local_sending_transactions,
        }
    }
    pub fn push_local_transaction(&mut self, transaction: &Transaction) -> ZingoLibResult<()> {
        let mut raw_tx = vec![];
        transaction
            .write(&mut raw_tx)
            .map_err(|e| ZingoLibError::CalculatedTransactionEncode(e.to_string()))?;
        self.local_sending_transactions.push(raw_tx);
        Ok(())
    }
    pub fn clear_local_transaction(&mut self) -> ZingoLibResult<()> {
        self.local_sending_transactions.clear();
        Ok(())
    }
    pub fn get_remote_txid_hashmap(&self) -> &HashMap<TxId, TransactionRecord> {
        self.remote_transactions
    }
    pub fn get_received_note_from_identifier(
        &self,
        note_record_reference: NoteRecordIdentifier,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteRecordIdentifier,
            zcash_client_backend::wallet::Note,
        >,
    > {
        let transaction = self.remote_transactions.get(&note_record_reference.txid);
        transaction
            .map(
                |transaction_record| match note_record_reference.shielded_protocol {
                    zcash_client_backend::ShieldedProtocol::Sapling => {
                        transaction_record
                            .get_received_note::<SaplingDomain>(note_record_reference.index)
                    }
                    zcash_client_backend::ShieldedProtocol::Orchard => {
                        transaction_record
                            .get_received_note::<OrchardDomain>(note_record_reference.index)
                    }
                },
            )
            .flatten()
    }
}

pub mod trait_inputsource;
