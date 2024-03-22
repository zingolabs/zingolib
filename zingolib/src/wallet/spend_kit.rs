use std::{convert::Infallible, num::NonZeroU32};

use crate::error::{ZingoLibError, ZingoLibResult};

use super::{
    data::WitnessTrees,
    record_book::{NoteRecordReference, RecordBook},
    transactions::TxMapAndMaybeTrees,
};
use nonempty::NonEmpty;
use sapling_crypto::prover::{OutputProver, SpendProver};
use zcash_client_backend::{
    data_api::{wallet::input_selection::GreedyInputSelector, InputSource},
    proposal::Proposal,
    wallet::OvkPolicy,
    zip321::TransactionRequest,
    ShieldedProtocol,
};
use zcash_keys::keys::UnifiedSpendingKey;

use zcash_primitives::{
    consensus,
    transaction::{fees::zip317::FeeRule as Zip317FeeRule, Transaction, TxId},
};
use zingoconfig::ChainType;

pub mod trait_inputsource;
pub mod trait_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

pub struct SpendKit<'book, 'trees> {
    pub key: UnifiedSpendingKey,
    pub params: ChainType,
    pub record_book: RecordBook<'book>,
    pub trees: &'trees mut WitnessTrees,
}

type GISKit<'a, 'b> = GreedyInputSelector<
    SpendKit<'a, 'b>,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

impl SpendKit<'_, '_> {
    pub fn create_proposal(
        &mut self,
        request: TransactionRequest,
    ) -> ZingoLibResult<Proposal<Zip317FeeRule, <Self as InputSource>::NoteRef>> {
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            Zip317FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
        );

        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );

        Ok(zcash_client_backend::data_api::wallet::propose_transfer::<
            SpendKit,
            ChainType,
            GISKit,
            ZingoLibError,
        >(
            self,
            &self.params.clone(),
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            NonZeroU32::new(1).expect("yeep yop"), //review! be more specific
        )
        .map_err(|e| ZingoLibError::Error(format!("{e:?}")))?) //review! error typing
    }
    pub fn calculate_transactions<Prover>(
        &mut self,
        sapling_prover: Prover,
        proposal: Proposal<Zip317FeeRule, <Self as InputSource>::NoteRef>,
    ) -> ZingoLibResult<NonEmpty<TxId>>
    where
        Prover: SpendProver + OutputProver,
    {
        zcash_client_backend::data_api::wallet::create_proposed_transactions::<
            SpendKit,
            ChainType,
            ZingoLibError,
            Zip317FeeRule,
            <Self as InputSource>::NoteRef, // note ref
        >(
            self,
            &self.params.clone(),
            &sapling_prover,
            &sapling_prover,
            &self.key.clone(),
            OvkPolicy::Sender,
            &proposal,
        )
        .map_err(|e| ZingoLibError::Error(format!("{e:?}"))) //review! error typing
    }
    pub fn propose_and_calculate<Prover>(
        &mut self,
        request: TransactionRequest,
        sapling_prover: Prover,
    ) -> ZingoLibResult<NonEmpty<TxId>>
    where
        Prover: SpendProver + OutputProver,
    {
        let proposal = self.create_proposal(request)?;
        self.calculate_transactions(sapling_prover, proposal)
    }
    pub fn get_calculated_transactions(&self) -> ZingoLibResult<Vec<Transaction>> {
        self.record_book.get_local_transactions()
    }
}
