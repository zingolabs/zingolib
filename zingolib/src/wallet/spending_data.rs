use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::{data::WitnessTrees, notes::NoteRecordIdentifier};

pub type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
pub type ShieldProposal = Proposal<FeeRule, Infallible>;
#[derive(Clone)]
pub enum ZingoProposal {
    Transfer(TransferProposal),
    Shield(ShieldProposal),
}

/// data that the spending wallet has, but viewkey wallet does not.
pub struct SpendingData {
    witness_trees: WitnessTrees,
    latest_proposal: Option<ZingoProposal>,
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
