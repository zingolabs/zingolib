use std::{collections::HashMap, convert::Infallible};

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::{fees::zip317::FeeRule, TxId};

use crate::wallet::data::{TransactionRecord, WitnessTrees};
use crate::wallet::notes::NoteRecordIdentifier;

pub struct TransactionRecordMap {
    pub map: HashMap<TxId, TransactionRecord>,
}

impl TransactionRecordMap {
    pub fn new_empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        Self { map }
    }
    pub fn get_received_note_from_identifier(
        &self,
        note_record_reference: NoteRecordIdentifier,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteRecordIdentifier,
            zcash_client_backend::wallet::Note,
        >,
    > {
        let transaction = self.map.get(&note_record_reference.txid);
        transaction.and_then(|transaction_record| match note_record_reference.pool {
            zcash_client_backend::PoolType::Transparent => None, // explain implication
            zcash_client_backend::PoolType::Shielded(domain) => match domain {
                zcash_client_backend::ShieldedProtocol::Sapling => transaction_record
                    .get_received_note::<SaplingDomain>(note_record_reference.index),
                zcash_client_backend::ShieldedProtocol::Orchard => transaction_record
                    .get_received_note::<OrchardDomain>(note_record_reference.index),
            },
        })
    }
}

pub mod trait_inputsource;

pub type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
pub type ShieldProposal = Proposal<FeeRule, Infallible>;
#[derive(Clone)]
pub enum ZingoProposal {
    Transfer(TransferProposal),
    Shield(ShieldProposal),
}

/// data that the spending wallet has, but viewkey wallet does not.
pub struct SpendingData {
    pub witness_trees: WitnessTrees,
    pub latest_proposal: Option<ZingoProposal>,
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
    pub current: TransactionRecordMap,
    pub spending_data: Option<SpendingData>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_spending() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap::new_empty(),
            spending_data: Some(SpendingData::default()),
        }
    }
    pub(crate) fn new_viewing() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap::new_empty(),
            spending_data: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.map.clear();
        self.spending_data.as_mut().map(SpendingData::clear);
    }
}
