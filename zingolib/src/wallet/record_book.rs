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

pub struct TransparentRecordRef {
    txid: TxId,
    index: u32,
}

pub struct RecordBook<'a> {
    remote_transactions: &'a HashMap<TxId, TransactionRecord>,
    local_raw_transactions: Vec<Vec<u8>>,
}

impl<'a> RecordBook<'a> {
    #[cfg(test)]
    pub fn new_empty() -> Self {
        let empty_map: HashMap<TxId, TransactionRecord> = HashMap::new();
        let empty_map_ref = Box::leak(Box::new(empty_map)); // Leak the empty hashmap to ensure its lifetime
        Self {
            remote_transactions: empty_map_ref,
            local_raw_transactions: Vec::new(),
        }
    }
    pub fn new_from_remote_txid_hashmap<'b>(
        remote_transactions: &'b HashMap<TxId, TransactionRecord>,
    ) -> Self
    where
        'b: 'a, // Ensure 'b outlives 'a
    {
        Self {
            remote_transactions,
            local_raw_transactions: Vec::new(),
        }
    }
    pub fn push_local_transaction(&mut self, transaction: &Transaction) -> ZingoLibResult<()> {
        let mut raw_tx = vec![];
        transaction
            .write(&mut raw_tx)
            .map_err(|e| ZingoLibError::CalculatedTransactionEncode(e.to_string()))?;
        self.local_raw_transactions.push(raw_tx);
        Ok(())
    }
    pub fn get_remote_txid_hashmap(&self) -> &HashMap<TxId, TransactionRecord> {
        self.remote_transactions
    }
    pub fn get_spendable_note_from_identifier(
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
    pub fn get_local_transactions(&self) -> ZingoLibResult<Vec<Transaction>> {
        let mut transactions = vec![];
        for raw_tx in &self.local_raw_transactions {
            transactions.push(
                Transaction::read(&raw_tx[..], zcash_primitives::consensus::BranchId::Canopy)
                    .map_err(|e| ZingoLibError::CalculatedTransactionDecode(e.to_string()))?,
            );
        }
        Ok(transactions)
    }
}

pub mod trait_inputsource;
