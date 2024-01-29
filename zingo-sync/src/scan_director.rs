//! Direct all scanning operations.
//!
//! The entry point for a lightclient synchronisation process
//! This module has a higher complexity and will likely be formed of many sub-modules
//!
//! Takes the wallet's keys, wallet options and birthday as input
//!
//! Load from wallet file and prepare scanning range:
//! Requests latest block height from proxy interface and constructs a scan range from birthday to
//! chaintip. zcash_client_backend may work well for this.
//! When a wallet's sync progress is saved it will continue from it's previous state. If it had
//! previously completed a sync, it will create a new scan range from the wallet's previously synced
//! height to the chaintip.
//! Create two more copies of this scan range
//!
//! Nullifer mapping:
//! Split the first scan range into batches for the nullifier mapper
//! Create nullifer mapper for scanning highest batch (scans highest to lowest)
//! When nullifier mapper completes, append returned hashmap to full nullifier map and save wallet
//! state. Is appending hashmaps the best option here? The concern is preservation of scan state
//! on reload
//! Create nullifier mapper for scanning second batch and repeat until all batches are scanned
//!
//! Compact block scanning:
//! Split the second scan range into batches for the trial decryptor
//! Create trial decryptor for scanning lowest batch (scans lowest to highest)
//! When trial decryptor completes, append returned list of TXIDs and save wallet state
//! Create trial decryptor for scanning second batch and repeat until all batches are scanned
//!
//! Transparent TXID fetching:
//! Split the third scan range into batches for the transparent TXID fetcher
//! Request TXIDs for all transparent inputs/outputs in the lowest batch from the proxy interface
//! Repeat for all batches
//!
//! Transaction scanning:
//! When the trial decryptor, transparent TXID fetcher or note returns a TXID, check it against
//! the TXID verification list and if its missing, add to the list and send the TXID to `priority`
//! for transaction scanning
//!
//! On reload:
//! Check wallet's transactions against the list of saved TXIDs and send the TXID of any missing
//! transactions to the transaction manager
//! Check last saved nullifier mapper batch and construct a new range from birthday (or lowest note?)
//! to last saved batch height
//! Check last saved trial decryptor batch and construct a new range from last saved batch height
//! to chaintip

mod priority;
