//! Direct all scanning operations.
//!
//! The entry point for a lightclient synchronization process.
//! This module has a higher complexity and will likely be formed of many sub-modules.
//!
//! Takes the wallet's keys, wallet options and birthday as input.
//!
//! Load from wallet file (if exists) and prepare/restore scanning range:
//! - When sync progress saved state exists it will continue from it's previous state. If it had
//! previously completed a sync, it will create a new scan range from the wallet's previously synced
//! height to the chaintip.
//! - If preparing new scan range, requests latest block height from proxy interface and constructs
//! a scan range from birthday to chaintip. zcash_client_backend may work well for this.
//! - If preparing new scan range, create two more copies of this scan range.
//!
//! Nullifer mapping:
//! - Split the first scan range into batches for the nullifier mapper. zcash_client_backend?
//! - Create nullifier mapper for scanning highest batch (scans highest to lowest)
//! - Create nullifier mapper for scanning second batch and repeat until all batches are scanned
//! On reload it should be safe to look at lowest height nullifier mapped and start from there,
//! including re-mapping the lowest height nullifier in case only some of the nullifiers at that height
//! were mapped.
//!
//! Compact block scanning:
//! - Split the second scan range into batches for the trial decryptor. zcash_client_backend?
//! - Create trial decryptor for scanning lowest batch (scans lowest to highest)
//! - Create trial decryptor for scanning second batch and repeat until all batches are scanned
//!
//! Transparent TXID fetching:
//! - Split the third scan range into batches for the transparent TXID fetcher. zcash_client_backend?
//! - Request TXIDs for all transparent inputs/outputs in the lowest batch from the proxy interface
//! - Request TXIDs for all transparent inputs/outputs in the second batch and repeat for all batches
//! Batches mitigate the queued TXIDs to `scan transaction` from getting too large, may not be neccessary.
//!
//! Transaction scanning:
//! - When `trial decrypt`, `transparent TXID fetcher` or `update spend` returns a TXID, add a copy to a
//! queued transactions list or hashmap and pass to `priority` to be ordered, then send to `scan
//! transaction`
//! - When `scan transaction` returns txid of transaction successfuly scanned, remove from queued
//! transactions
//!
//! On reload:
//! Queued transactions list/hashmap will ensure that when wallet is saved that one of three cases
//! will keep wallet state correct
//! 1. Save was called before, for example, `trial decrypt` finished it's batch and wallet
//! crashed/closed. In this case, `trial decrypt` would re-scan the incomplete batch on reload.
//! 2. Save was called after, for example, `trial decrypt` completed a batch, but the wallet
//! crashed/closed before the transaction was scanned. In this case, the queued transaction would
//! be stored and re-send to `scan transaction` on reload.
//! 3. Save was called after the transaction was scanned. In this case, the basic transaction and
//! notes would be stored in the wallet.

mod priority;
