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
use spending_data::SpendingData;
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;
use zcash_primitives::legacy::TransparentAddress;

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMap {
    /// TODO: Doc-comment!
    pub transaction_records_by_id: TransactionRecordsById,
    spending_data: Option<SpendingData>,
    pub(crate) transparent_child_addresses:
        Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
}

pub mod get;
pub mod read_write;
pub mod recording;

pub mod spending_data;

impl TxMap {
    pub(crate) fn new_with_witness_trees(
        transparent_child_addresses: Arc<
            append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>,
        >,
    ) -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(WitnessTrees::default())),
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
            spending_data: None,
            transparent_child_addresses,
        }
    }
    /// TODO: Doc-comment!
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.spending_data
            .as_ref()
            .map(|spending_data| spending_data.witness_trees())
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.spending_data
            .as_mut()
            .map(|spending_data| spending_data.witness_trees_mut())
    }
    /// TODO: Doc-comment!
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.witness_trees_mut().map(WitnessTrees::clear);
    }
}
#[cfg(test)]
impl TxMap {
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_with_witness_trees_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(WitnessTrees::default())),
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_treeless_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: None,
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum TxMapTraitError {
    #[error("No witness trees. This is viewkey watch, not a spendkey wallet.")]
    NoSpendCapability,
    #[error("{0:?}")]
    InputSource(InputSourceError),
    #[error("{0:?}")]
    TransactionWrite(std::io::Error),
}

pub mod trait_stub_inputsource;
pub mod trait_stub_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;
