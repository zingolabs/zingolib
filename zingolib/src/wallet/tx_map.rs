//! This mod should be called tx_map_and_maybe_trees.rs. it contains
//! struct TxMap
//! implementations for TxMap
//! associated types for TxMap that have no relevance elsewhere.

use crate::{
    data::witness_trees::WitnessTrees,
    wallet::{
        error::KeyError,
        transaction_records_by_id::{trait_inputsource::InputSourceError, TransactionRecordsById},
    },
};
use getset::{Getters, MutGetters};
use spending_data::SpendingData;
use std::{fmt::Debug, sync::Arc};
use zcash_client_backend::wallet::TransparentAddressMetadata;
use zcash_primitives::legacy::{keys::EphemeralIvk, TransparentAddress};

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
    /// rejection_addresses are called "ephemeral" in LRZ 320
    pub(crate) rejection_addresses:
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
        rejection_addresses: Arc<
            append_only_vec::AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>,
        >,
        rejection_ivk: EphemeralIvk,
    ) -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(WitnessTrees::default(), rejection_ivk)),
            transparent_child_addresses,
            rejection_addresses,
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
            rejection_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
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
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum TxMapTraitError {
    #[error("No witness trees. This is viewkey watch, not a spendkey wallet.")]
    NoSpendCapability,
    #[error("{0:?}")]
    InputSource(InputSourceError),
    #[error("{0:?}")]
    TransactionWrite(std::io::Error),
    #[error("{0}")]
    TexSendError(KeyError),
}

pub mod trait_stub_inputsource;
pub mod trait_stub_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

#[cfg(test)]
impl TxMap {
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_with_witness_trees_address_free() -> TxMap {
        // The first 32 bytes are a BIP32/44 chain code, the subsequent 33 bytes are a pubkey
        // https://github.com/zancas/zcash-test-vectors/blob/db9c9b9519a32859a46bbbc60368e8741fe629c4/test-vectors/rust/zip_0316.rs#L10
        const EXTENDED_PUBKEY: [u8; 65] = [
            0x5d, 0x7a, 0x8f, 0x73, 0x9a, 0x2d, 0x9e, 0x94, 0x5b, 0x0c, 0xe1, 0x52, 0xa8, 0x04,
            0x9e, 0x29, 0x4c, 0x4d, 0x6e, 0x66, 0xb1, 0x64, 0x93, 0x9d, 0xaf, 0xfa, 0x2e, 0xf6,
            0xee, 0x69, 0x21, 0x48, 0x02, 0x16, 0x88, 0x4f, 0x1d, 0xbc, 0x92, 0x90, 0x89, 0xa4,
            0x17, 0x6e, 0x84, 0x0b, 0xb5, 0x81, 0xc8, 0x0e, 0x16, 0xe9, 0xb1, 0xab, 0xd6, 0x54,
            0xe6, 0x2c, 0x8b, 0x0b, 0x95, 0x70, 0x20, 0xb7, 0x48,
        ];
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: Some(SpendingData::new(
                WitnessTrees::default(),
                zcash_primitives::legacy::keys::AccountPubKey::deserialize(&EXTENDED_PUBKEY)
                    .unwrap()
                    .derive_ephemeral_ivk()
                    .unwrap(),
            )),
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
            rejection_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
    /// For any unit tests that don't require a WalletCapability, where the addresses come from
    pub(crate) fn new_treeless_address_free() -> TxMap {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            spending_data: None,
            transparent_child_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
            rejection_addresses: Arc::new(append_only_vec::AppendOnlyVec::new()),
        }
    }
}
