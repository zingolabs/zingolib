use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::data::{TransactionRecord, WitnessTrees};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct ZingoLedger {
    pub current: HashMap<TxId, TransactionRecord>,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod inputsource;
pub mod recording;
pub mod serial_read_write;

impl ZingoLedger {
    pub(crate) fn new_with_witness_trees() -> ZingoLedger {
        Self {
            current: HashMap::default(),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> ZingoLedger {
        Self {
            current: HashMap::default(),
            witness_trees: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::{
        address::Address,
        data_api::wallet::input_selection::GreedyInputSelector,
        fees::{self, standard::SingleOutputChangeStrategy, ChangeStrategy, DustOutputPolicy},
        zip321::{self, Payment},
    };
    use zcash_primitives::transaction::{
        components::amount::NonNegativeAmount, fees::StandardFeeRule,
    };
    use zingoconfig::ChainType;

    use super::ZingoLedger;

    #[test]
    fn test() {
        let amount = NonNegativeAmount::const_from_u64(20000);
        let recipient_address =
            Address::decode(&ChainType::Mainnet, &"squirrel".to_string()).unwrap();
        let request = zip321::TransactionRequest::new(vec![Payment {
        recipient_address,
        amount,
        memo: None,
        label: None,
        message: None,
        other_params: vec![],
    }])
    .expect(
        "It should not be possible for this to violate ZIP 321 request construction invariants.",
    );

        let change_strategy = SingleOutputChangeStrategy::new(StandardFeeRule::Zip317, None);
        let input_selector = GreedyInputSelector::<ZingoLedger, _>::new(
            change_strategy,
            DustOutputPolicy::default(),
        );

        // propose_transfer(
        //     wallet_db,
        //     params,
        //     spend_from_account,
        //     &input_selector,
        //     request,
        //     min_confirmations,
        // )
    }
}
