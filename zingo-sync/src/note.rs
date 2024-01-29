//! Data structures for notes.
//!
//! Defines orchard notes, sapling notes and transparent notes
//! Includes traits for any shared data or functionality
//!
//! Includes functionality for iterating through all transparent notes and checking if they match
//! the transparent inputs in existing transactions in the wallet (transparent input mapper?).
//!
//! Also includes functionality for iterating through all shielded notes and checking if the
//! calculated nullifier matches a nullifier in the nullifier map to mark note as spent.
//! When a note is marked spent, send the TXID of the transaction the note was consumed to
//! `scan director`, otherwise a sending transaction may not be created in the case where
//! no change was received.
//! This also has the added benefit of "rifts" where a newly created note is immediately linked
//! to the transaction it was consumed in via the nullifier map and can enable targetted
//! transaction scanning, similar to (or maybe _IS_) DAG sync.
