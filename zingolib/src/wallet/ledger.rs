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
pub mod walletcommitmenttrees;
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

    use incrementalmerkletree::Position;
    use orchard::{
        note::{Nullifier, RandomSeed},
        note_encryption::OrchardDomain,
        value::NoteValue,
    };
    use zcash_client_backend::{
        address::Address,
        data_api::wallet::input_selection::GreedyInputSelector,
        fees::DustOutputPolicy,
        zip321::{self, Payment},
        ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight,
        transaction::{components::amount::NonNegativeAmount, TxId},
    };
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingoconfig::ChainType;

    use crate::error::ZingoLibError;

    use super::ZingoLedger;

    #[test]
    fn test_propose_transfer() {
        let change_strategy = zcash_client_backend::fees::standard::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::StandardFeeRule::Zip317,
            None,
            ShieldedProtocol::Orchard,
        );
        let input_selector = GreedyInputSelector::<ZingoLedger, _>::new(
            change_strategy,
            DustOutputPolicy::default(),
        );

        let min_confirmations = NonZeroU32::new(10).unwrap();

        let mut ledger = ZingoLedger::new_treeless();
        // assert_eq!(
        //     &ledger
        //         .get_target_and_anchor_heights(min_confirmations)
        //         .unwrap(),
        //     None
        // );

        dbg!("inventing transaction");
        let txid = TxId::from_bytes([0; 32]);
        let status = ConfirmationStatus::Confirmed(BlockHeight::from(100));
        let timestamp = 123456;

        dbg!("inventing nullifier");
        let nullifier = Nullifier::from_bytes(&[0; 32]).unwrap();
        dbg!("inventing address");
        let internal_orchard_address = orchard::Address::from_raw_address_bytes(&[7; 43]).unwrap(); //this vector of 7s happens to work
        dbg!("inventing note");
        let note = {
            let random_seed = RandomSeed::from_bytes([0; 32], &nullifier).unwrap();
            orchard::note::Note::from_parts(
                internal_orchard_address,
                NoteValue::from_raw(40000),
                nullifier,
                random_seed,
            )
            .unwrap()
        };
        dbg!("adding note");
        let position = Position::from(100);
        ledger.add_new_note::<OrchardDomain>(
            txid,
            status,
            timestamp,
            note,
            internal_orchard_address,
            true,
            Some(nullifier),
            0,
            position,
        );

        let request_amount = NonNegativeAmount::const_from_u64(20000);
        let recipient_address =
            Address::decode(&ChainType::Testnet, &"utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05".to_string()).unwrap();
        let request = zip321::TransactionRequest::new(vec![Payment {
            recipient_address,
            amount: request_amount,
            memo: None,
            label: None,
            message: None,
            other_params: vec![],
        }])
        .expect(
            "It should not be possible for this to violate ZIP 321 request construction invariants.",);

        dbg!("proposing transfer");
        let proposal = zcash_client_backend::data_api::wallet::propose_transfer::<
            ZingoLedger,
            ChainType,
            GreedyInputSelector<
                ZingoLedger,
                zcash_client_backend::fees::standard::SingleOutputChangeStrategy,
            >,
            ZingoLibError,
        >(
            &mut ledger,
            &ChainType::Testnet,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            min_confirmations,
        )
        .unwrap();
        dbg!(proposal);
    }
}
