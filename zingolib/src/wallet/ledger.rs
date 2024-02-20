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
pub mod walletread;

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
    use std::num::NonZeroU32;

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
    fn test_propose_transfer() {
        let amount = NonNegativeAmount::const_from_u64(20000);
        let recipient_address =
            Address::decode(&ChainType::Testnet, &"utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05".to_string()).unwrap();
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

        let min_confirmations = NonZeroU32::new_unchecked(10);
        todo!();
        zcash_client_backend::data_api::wallet::propose_transfer(
            wallet_db,
            params,
            spend_from_account,
            &input_selector,
            request,
            min_confirmations,
        );
    }
}
