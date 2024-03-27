use std::num::NonZeroU32;

use crate::error::{ZingoLibError, ZingoLibResult};

use self::errors::CreateTransactionsError;

use super::{data::WitnessTrees, record_book::RefRecordBook, transactions::Proposa};
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

use zcash_primitives::transaction::{fees::zip317::FeeRule as Zip317FeeRule, TxId};
use zingoconfig::ChainType;

pub mod trait_inputsource;
pub mod trait_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

pub mod errors;

pub struct SpendKit<'book, 'trees> {
    pub key: UnifiedSpendingKey,
    pub params: ChainType,
    pub record_book: RefRecordBook<'book>,
    pub trees: &'trees mut WitnessTrees,
    pub latest_proposal: &'trees mut Option<Proposa>,
    // review! how do we actually recognize this as canon when selecting?
    pub local_sending_transactions: Vec<Vec<u8>>,
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
        ); // review consider change strategy!

        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );

        let proposal = zcash_client_backend::data_api::wallet::propose_transfer::<
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
        .map_err(|e| ZingoLibError::ProposeTransaction(format!("{}", e)))?;

        *self.latest_proposal = Some(proposal.clone());
        Ok(proposal)
        //review! error typing
    }
    pub fn create_transactions<Prover>(
        &mut self,
        sapling_prover: Prover,
    ) -> Result<&Vec<Vec<u8>>, CreateTransactionsError>
    where
        Prover: SpendProver + OutputProver,
    {
        if let Some(proposal) = self.latest_proposal.clone() {
            let _txids = zcash_client_backend::data_api::wallet::create_proposed_transactions::<
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
            .map_err(|e| CreateTransactionsError::CreateTransactions(e))?;

            Ok(&self.local_sending_transactions)
        } else {
            Err(CreateTransactionsError::NoProposal)
        }
        //review! error typing
    }
}
