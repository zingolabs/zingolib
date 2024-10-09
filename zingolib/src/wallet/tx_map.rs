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
use getset::{Getters, MutGetters};
use spending_data::SpendingData;
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;
use zcash_client_backend::wallet::TransparentAddressMetadata;
use zcash_primitives::legacy::{
    keys::{self, EphemeralIvk},
    TransparentAddress,
};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
#[derive(Getters, MutGetters)]
pub struct TxMap {
    /// TODO: Doc-comment!
    pub transaction_records_by_id: TransactionRecordsById,
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    spending_data: Option<SpendingData>,
    // as below
    pub(crate) transparent_child_addresses:
        Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
    // TODO: rename (ephemeral_transparent_addresses?)
    pub(crate) transparent_child_ephemeral_addresses:
        Arc<append_only_vec::AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>>,
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
        transparent_child_ephemeral_addresses: Arc<
            append_only_vec::AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>,
        >,
        transparent_ephemeral_ivk: EphemeralIvk,
    ) -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(
                WitnessTrees::default(),
                transparent_ephemeral_ivk,
            )),
            transparent_child_addresses,
            transparent_child_ephemeral_addresses,
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
            transparent_child_ephemeral_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
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
    /// Generate a new ephemeral transparent address,
    /// for use in a send to a TEX address.
    pub fn new_ephemeral_address(
        &self,
    ) -> Result<
        (
            zcash_primitives::legacy::TransparentAddress,
            zcash_client_backend::wallet::TransparentAddressMetadata,
        ),
        String,
    > {
        let child_index = keys::NonHardenedChildIndex::from_index(
            self.transparent_child_ephemeral_addresses.len() as u32,
        )
        .ok_or_else(|| String::from("Ephemeral index overflow"))?;
        let t_addr = self
            .spending_data()
            .as_ref()
            .ok_or_else(|| String::from("Ephemeral addresses are only generated at spend time"))?
            .transparent_ephemeral_ivk()
            .derive_ephemeral_address(child_index)
            .map_err(|e| e.to_string())?;
        self.transparent_child_ephemeral_addresses.push((
            t_addr,
            TransparentAddressMetadata::new(
                keys::TransparentKeyScope::EPHEMERAL,
                keys::NonHardenedChildIndex::from_index(
                    self.transparent_child_ephemeral_addresses.len() as u32,
                )
                .expect("ephemeral index overflow"),
            ),
        ));
        Ok(self
            .transparent_child_ephemeral_addresses
            .iter()
            .last()
            .expect("we just generated an address, this is known to be non-empty")
            .clone())
    }
}
#[cfg(test)]
impl TxMap {
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_with_witness_trees_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(
                WitnessTrees::default(),
                keys::AccountPubKey::deserialize(&[0; 65])
                    .unwrap()
                    .derive_ephemeral_ivk()
                    .unwrap(),
            )),
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
            transparent_child_ephemeral_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_treeless_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: None,
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
            transparent_child_ephemeral_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
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
    #[error("{0}")]
    TexSendError(String),
}

pub mod trait_stub_inputsource;
pub mod trait_stub_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;
