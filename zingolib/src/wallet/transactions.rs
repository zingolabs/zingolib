use std::collections::HashMap;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::{fees::zip317::FeeRule, TxId};

use super::{
    data::{TransactionRecord, WitnessTrees},
    record_book::NoteRecordIdentifier,
};

pub type Proposa = Proposal<FeeRule, NoteRecordIdentifier>;

/// data that the spending wallet has, but viewkey wallet does not.
pub struct SpendingData {
    pub witness_trees: WitnessTrees,
    pub latest_proposal: Option<Proposa>,
    // only one outgoing send can be proposed at once. the first vec contains steps, the second vec is raw bytes.
    // pub outgoing_send_step_data: Vec<Vec<u8>>,
}

impl SpendingData {
    pub fn default() -> Self {
        SpendingData {
            witness_trees: WitnessTrees::default(),
            latest_proposal: None,
        }
    }
    pub fn load_with_option_witness_trees(
        option_witness_trees: Option<WitnessTrees>,
    ) -> Option<Self> {
        option_witness_trees.map(|witness_trees| SpendingData {
            witness_trees,
            latest_proposal: None,
        })
    }
    pub fn clear(&mut self) {
        *self = Self::default()
    }
}

/// Transactions Metadata and Maybe Trees
/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub current: HashMap<TxId, TransactionRecord>, //soon to be superceded to RecordBook, which will inherit any methods from this struct.
    pub spending_data: Option<SpendingData>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_spending() -> TxMapAndMaybeTrees {
        Self {
            current: HashMap::default(),
            spending_data: Some(SpendingData::default()),
        }
    }
    pub(crate) fn new_viewing() -> TxMapAndMaybeTrees {
        Self {
            current: HashMap::default(),
            spending_data: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.clear();
        self.spending_data.as_mut().map(SpendingData::clear);
    }
}
