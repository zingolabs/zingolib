//! Depricated! holds sync engine 1.0

pub(super) mod block_management_reorg_detection;
pub(super) mod fetch_compact_blocks;
pub(super) mod fetch_taddr_transactions;
pub(super) mod full_transactions_processor;
pub(super) mod sync_status;
pub(super) mod syncdata;
pub(super) mod targetted_rescan;
pub(super) mod trial_decryptions;
pub(super) mod update_notes;

#[cfg(test)]
pub mod test_utils;
