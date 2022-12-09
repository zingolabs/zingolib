use core::fmt;
use std::collections::HashMap;

use crate::wallet::data::PoolNullifier;

#[derive(Clone, Debug, Default)]
pub struct BatchSyncStatus {
    pub in_progress: bool,
    pub last_error: Option<String>,

    pub sync_id: u64,
    pub start_block: u64,
    pub end_block: u64,

    pub blocks_done: u64,
    pub trial_dec_done: u64,
    pub txn_scan_done: u64,

    pub witnesses_updated: HashMap<PoolNullifier, u64>,

    pub blocks_total: u64,

    pub batch_num: usize,
    pub batch_total: usize,
}

impl BatchSyncStatus {
    pub fn start_new(&mut self, batch_total: usize) {
        log::debug!("BatchSyncStatus::start_new(num_blocks_in_batch) called!");
        self.sync_id += 1;
        self.last_error = None;
        self.in_progress = true;
        self.blocks_done = 0;
        self.trial_dec_done = 0;
        self.blocks_total = 0;
        self.txn_scan_done = 0;
        self.witnesses_updated = HashMap::new();
        self.batch_num = 0;
        self.batch_total = batch_total;
    }

    /// Setup a new sync status in prep for an upcoming sync
    pub fn new_sync_batch(&mut self, start_block: u64, end_block: u64, batch_num: usize) {
        log::debug!(
            "BatchSyncStatus::new_sync_batch(
            start_block: {start_block}, end_block: {end_block}, batch_num: {batch_num}
            ) called!"
        );
        self.in_progress = true;
        self.last_error = None;

        self.start_block = start_block;
        self.end_block = end_block;
        self.blocks_done = 0;
        self.trial_dec_done = 0;
        self.blocks_total = 0;
        self.witnesses_updated = HashMap::new();
        self.txn_scan_done = 0;
        self.batch_num = batch_num;
    }

    /// Finish up a sync
    pub fn finish(&mut self) {
        self.in_progress = false;
    }
}

impl fmt::Display for BatchSyncStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.blocks_total > 0 && self.in_progress {
            write!(
                f,
                "**Batch** Current: {:4} Total: {:4}\n   \
                Blocks Loaded: {:4} TrialDecrypted: \
                {:4}, Witnesses Updated: {:4}, Total: {:4}, ",
                self.batch_num,
                self.batch_total,
                self.blocks_done,
                self.trial_dec_done,
                self.witnesses_updated.values().min().unwrap_or(&0),
                self.blocks_total
            )
        } else {
            write!(
                f,
                "id: {}, total_batch_blocks: {:4}, in_progress: {}, errors: {}",
                self.sync_id,
                self.blocks_total,
                self.in_progress,
                self.last_error.as_ref().unwrap_or(&"None".to_string())
            )
        }
    }
}
