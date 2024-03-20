use std::{convert::Infallible, num::NonZeroU32};

use crate::error::ZingoLibError;

use super::{data::WitnessTrees, record_book::RecordBook, transactions::TxMapAndMaybeTrees};
use sapling_crypto::prover::{OutputProver, SpendProver};
use zcash_client_backend::{
    data_api::{wallet::input_selection::GreedyInputSelector, InputSource},
    proposal::Proposal,
    wallet::OvkPolicy,
    zip321::TransactionRequest,
    ShieldedProtocol,
};
use zcash_keys::keys::UnifiedSpendingKey;

use zcash_primitives::{consensus, transaction::fees::zip317::FeeRule as Zip317FeeRule};
use zingoconfig::ChainType;

pub mod trait_inputsource;
pub mod trait_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

pub struct SpendKit<'a> {
    pub key: UnifiedSpendingKey,
    pub params: ChainType,
    pub record_book: RecordBook<'a>,
    pub trees: &'a WitnessTrees,
}

type GISKit<'a> = GreedyInputSelector<
    SpendKit<'a>,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

impl SpendKit<'_> {
    pub fn create_proposal(
        &mut self,
        request: TransactionRequest,
    ) -> Result<Proposal<Zip317FeeRule, <Self as InputSource>::NoteRef>, ZingoLibError> {
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
        .map_err(|e| ZingoLibError::UnknownError)?) //review! error typing
    }
    pub fn create_transactions<Prover>(
        &mut self,
        sapling_prover: Prover,
        proposal: Proposal<Zip317FeeRule, u32>,
    ) -> ()
    where
        Prover: SpendProver + OutputProver,
    {
        let _ = zcash_client_backend::data_api::wallet::create_proposed_transactions::<
            SpendKit,
            ChainType,
            ZingoLibError,
            Zip317FeeRule,
            u32, // note ref
        >(
            self,
            &self.params.clone(),
            &sapling_prover,
            &sapling_prover,
            &self.key.clone(),
            OvkPolicy::Sender,
            &proposal,
        );
    }
}
