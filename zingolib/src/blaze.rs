//! Depricated! holds sync engine 1.0

pub(super) mod block_management_reorg_detection;
pub(super) mod fetch_compact_blocks;
pub(super) mod fetch_taddr_transactions;
/// alternative name: daemon_for_txid_lookup_and_record_updates
/// this function can update a few details about a TransactionRecord. it has numerous gaps
/// It is the closest thing Zingo has to conditional rescan. it needs to be conditional rescan. as of this git commit 41cf89b it is not.
pub(super) mod full_transactions_processor;
pub(super) mod sync_status;
pub(super) mod syncdata;
pub(super) mod trial_decryptions;
pub(super) mod update_notes;

#[cfg(test)]
pub mod test_utils;
