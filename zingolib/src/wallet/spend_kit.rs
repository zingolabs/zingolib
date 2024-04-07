use std::{convert::Infallible, num::NonZeroU32, ops::DerefMut, sync::Arc};

use crate::error::{ZingoLibError, ZingoLibResult};

use self::errors::CreateTransactionsError;

use super::{
    data::WitnessTrees,
    keys::unified::WalletCapability,
    record_book::RefRecordBook,
    transactions::{Proposa, TxMapAndMaybeTrees},
    LightWallet,
};
use nonempty::NonEmpty;
use sapling_crypto::prover::{OutputProver, SpendProver};
use tokio::sync::RwLockWriteGuard;
use zcash_client_backend::{
    data_api::{wallet::input_selection::GreedyInputSelector, InputSource},
    proposal::Proposal,
    wallet::OvkPolicy,
    zip321::TransactionRequest,
    ShieldedProtocol,
};
use zcash_keys::keys::UnifiedSpendingKey;

use zcash_primitives::{
    legacy::keys::pubkey_to_address,
    transaction::{
        components::amount::NonNegativeAmount, fees::zip317::FeeRule as Zip317FeeRule, TxId,
    },
};
use zingoconfig::ChainType;
use zip32::AccountId;

pub mod trait_inputsource;
pub mod trait_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

pub mod errors;

pub struct SpendKit<'book, 'trees> {
    pub spend_cap: Arc<WalletCapability>,
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
    pub async fn assemble<'lock, 'reflock, 'trees, 'book>(
        wallet: &'lock LightWallet,
        context_write_lock: &'reflock mut RwLockWriteGuard<'lock, TxMapAndMaybeTrees>,
    ) -> ZingoLibResult<SpendKit<'book, 'trees>>
    where
        'lock: 'trees + 'book,
        'reflock: 'trees + 'book,
    {
        if let TxMapAndMaybeTrees {
            spending_data: Some(spending_data),
            current: all_remote_transactions,
        } = context_write_lock.deref_mut()
        {
            Ok(SpendKit::<'book, 'trees> {
                spend_cap: wallet.wallet_capability(),
                params: wallet.transaction_context.config.chain,
                record_book: RefRecordBook::new_from_remote_txid_hashmap(all_remote_transactions), //review! if there are already pending transactions, dont assemble a spend_kit
                trees: &mut spending_data.witness_trees,
                latest_proposal: &mut spending_data.latest_proposal,
                local_sending_transactions: Vec::new(),
            })
        } else {
            Err(ZingoLibError::ViewkeyCantSpend)
        }
    }
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

        *self.latest_proposal = Some(Proposa::Transfer(proposal.clone()));
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
            match proposal {
                Proposa::Transfer(prop) => {
                    let _txids =
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
                            &UnifiedSpendingKey::try_from(self.spend_cap.as_ref())
                                .map_err(|e| CreateTransactionsError::CannotSpend(e.to_string()))?,
                            OvkPolicy::Sender,
                            &prop,
                        )
                        .map_err(|e| ZingoLibError::CalculateTransaction(format!("{e:?}")));

                    Ok(&self.local_sending_transactions)
                }
                Proposa::Shield(prop) => {
                    let _txids =
                        zcash_client_backend::data_api::wallet::create_proposed_transactions::<
                            SpendKit,
                            ChainType,
                            ZingoLibError,
                            Zip317FeeRule,
                            Infallible,
                        >(
                            self,
                            &self.params.clone(),
                            &sapling_prover,
                            &sapling_prover,
                            &UnifiedSpendingKey::try_from(self.spend_cap.as_ref())
                                .map_err(|e| CreateTransactionsError::CannotSpend(e.to_string()))?,
                            OvkPolicy::Sender,
                            &prop,
                        )
                        .map_err(|e| ZingoLibError::CalculateTransaction(format!("{e:?}")));

                    Ok(&self.local_sending_transactions)
                }
            }
        } else {
            Err(CreateTransactionsError::NoProposal)
        }
    }

    pub fn propose_shielding(&mut self) -> ZingoLibResult<Proposal<Zip317FeeRule, Infallible>> {
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            Zip317FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
        ); // review consider change strategy!

        let secp = secp256k1::Secp256k1::new();
        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );
        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            SpendKit,
            ChainType,
            GISKit,
            ZingoLibError,
        >(
            self,
            &self.params.clone(),
            &input_selector,
            //review! how much?? configurable?
            NonNegativeAmount::const_from_u64(10_000),
            &self
                .spend_cap
                .transparent_child_keys()
                .expect("review! fix this expect")
                .iter()
                .map(|(_index, sk)| pubkey_to_address(&sk.public_key(&secp)))
                .collect::<Vec<_>>(),
            // review! do we want to require confirmations?
            // make it configurable?
            0,
        )
        .map_err(|e| ZingoLibError::ProposeTransaction(format!("{}", e)))?;

        *self.latest_proposal = Some(Proposa::Shield(proposed_shield.clone()));
        Ok(proposed_shield)
    }
}
