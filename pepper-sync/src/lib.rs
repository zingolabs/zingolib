#![warn(missing_docs)]
//! Pepper Sync
//!
//! Pepper-sync is a crate providing a sync engine for the zcash network providing the following features:
//! - Non-linear scanning, allowing chain tip and targetted scanning
//! - Spend-before-sync, combines the shard roots with high priority chain tip scanning to enable spending of notes as
//!   they are scanned
//! - Speed, trial decryption and tree building are computed in parallel and a multi-task architecture help maximumise
//!   throughput for fetching and scanning
//! - Scan by shards, uses subtree metadata to create scan ranges that contain all note commitments to each shard to
//!   enable faster spending of decrypted outputs.
//! - Fixed memory batching, each scan worker receives a batch with a fixed number of outputs for stable memory usage.
//! - Pause and resume, the sync engine can be paused to allow the wallet to perform time-critical tasks that would
//!   require the acquisition of the wallet lock multiple times in quick succession.
//!
//! Terminology:
//! - Chain height - highest block height of best chain from the server.
//! - Wallet height - highest block height of blockchain known to the wallet.
//! - Fully scanned height - block height in which the wallet has completed scanning all blocks equal to and below this height.
//! - Shard range - the range of blocks that contain all note commitments to a fully completed shard in the blockchain.
//!
//! Initialisation:
//! 1. For first time sync, creates a single scan range from birthday to chain height.
//! 2. If synced previously, merge all previously scanned ranges together and create a new scan range from wallet
//!    height to chain height (the part of the chain that has been mined since last sync).
//! 3. Use locators from transparent address discovery and targetted scanning to set "found note" priority ranges.
//! 4. Finds the upper range bound of the latest orchard and sapling shard ranges and splits the scan range at the lowest
//!    height of the two, setting the upper scan range to priority "chain tip". This ensures that both the sapling and orchard
//!    note commitments are scanned in the latest incomplete shard at the chain tip.
//! 5. Set the first 10 blocks after the highest previously scanned blocks to "verify" priority to check for re-org.
//!
//! Scanning:
//! 1. Take the highest priority scan range and split off an orchard shard range off the lower end. setting to "ignored"
//!    (a.k.a scanning) priority and sending this to the "batcher" task.
//! 2. The batcher takes this scan range (containing all note commitments to an orchard shard) and fetches the compact
//!    blocks, splitting the blocks into multiple batches with a fixed number of outputs.
//! 3. Each batch is sent to an idle "scan worker" which scans a batch and returns scan results to the main sync process.
//! 4. When the scan results have been processed the scan range containing the batch of scanned blocks is set to "scanned"
//!    priority.
//! 5. When a worker is scanning the final batch in a scan range (shard range), the batcher will go idle and the main task
//!    will send it a new scan range of highest priority. it will then send the first batch of this new scan range to an idle
//!    scan worker and the scan process continues until all ranges are scanned.
//! 6. When scan results are being processed, it checks newly derived nullifiers of decrypted outputs against the nullifier
//!    map (a map of all nullifiers in the transactions inputs/spends on chain). If a spend is found it sets the surrounding
//!    shard range (sapling shard if sapling note is spent and orchard shard if orchard note is spent) to "found note" priority.
//!

pub(crate) mod client;
pub mod error;
pub mod keys;
pub(crate) mod scan;
pub mod sync;
pub mod wallet;
pub(crate) mod witness;

pub use sync::add_scan_targets;
pub use sync::scan_pending_transaction;
pub use sync::sync;
pub use sync::sync_status;

pub(crate) const MAX_BATCH_OUTPUTS: usize = 2usize.pow(12);
