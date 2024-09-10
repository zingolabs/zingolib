//! This mod should be called tx_map_and_maybe_trees.rs. it contains
//! struct TxMap
//! implementations for TxMap
//! associated types for TxMap that have no relevance elsewhere.

use crate::{
    data::witness_trees::WitnessTrees,
    wallet::transaction_records_by_id::{
        trait_inputsource::InputSourceError, TransactionRecordsById,
    },
};
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;
use zcash_primitives::legacy::TransparentAddress;

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMap {
    /// TODO: Doc-comment!
    pub transaction_records_by_id: TransactionRecordsById,
    witness_trees: Option<WitnessTrees>,
    pub(crate) transparent_child_addresses:
        Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMap {
    pub(crate) fn new_with_witness_trees(
        transparent_child_addresses: Arc<
            append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>,
        >,
    ) -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: Some(WitnessTrees::default()),
            transparent_child_addresses,
        }
    }
    pub(crate) fn new_treeless(
        transparent_child_addresses: Arc<
            append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>,
        >,
    ) -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: None,
            transparent_child_addresses,
        }
    }
    /// TODO: Doc-comment!
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.witness_trees.as_ref()
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.witness_trees.as_mut()
    }
    /// TODO: Doc-comment!
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
#[cfg(test)]
impl TxMap {
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_with_witness_trees_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: Some(WitnessTrees::default()),
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_treeless_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: None,
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
}

/// TODO: Doc-comment!
#[derive(Debug, PartialEq, Error)]
pub enum TxMapTraitError {
    /// TODO: Doc-comment!
    #[error("No witness trees. This is viewkey watch, not a spendkey wallet.")]
    NoSpendCapability,
    /// TODO: Doc-comment!
    #[error("{0:?}")]
    InputSource(InputSourceError),
}

pub mod trait_stub_inputsource;
pub mod trait_stub_walletcommitmenttrees;
pub mod trait_walletread;
