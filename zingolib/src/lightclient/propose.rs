//! LightClient function do_propose generates a proposal to send to specified addresses.

use crate::{
    lightclient::LightClient,
    wallet::tx_map_and_maybe_trees::{TxMapAndMaybeTrees, TxMapAndMaybeTreesTraitError},
};
use std::{convert::Infallible, num::NonZeroU32, ops::DerefMut};
use thiserror::Error;
use zcash_client_backend::{
    data_api::wallet::input_selection::GreedyInputSelector,
    zip321::{Payment, TransactionRequest, Zip321Error},
    ShieldedProtocol,
};
use zcash_keys::address::Address;
use zcash_primitives::{
    legacy::keys::pubkey_to_address,
    memo::MemoBytes,
    transaction::components::amount::{BalanceError, NonNegativeAmount},
};
use zingoconfig::ChainType;

/// Errors that can result from do_propose
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum RawToTransactionRequestError {
    #[error("Could not parse address.")]
    Address,
    #[error("Invalid amount: {0}")]
    Amount(BalanceError),
    #[error("Invalid memo: {0}")]
    Memo(zcash_primitives::memo::Error),
    #[error("Error requesting transaction: {0}")]
    Zip321(Zip321Error),
}

impl LightClient {
    /// takes raw data as input (strings and numbers) and returns a TransactionRequest
    pub fn raw_to_transaction_request(
        &self,
        address_amount_memo_tuples: Vec<(String, u32, Option<String>)>,
    ) -> Result<TransactionRequest, RawToTransactionRequestError> {
        let mut payments = vec![];
        for receiver in address_amount_memo_tuples {
            let recipient_address = Address::decode(
                &self.wallet.transaction_context.config.chain,
                receiver.0.as_str(),
            )
            .ok_or(RawToTransactionRequestError::Address)?;

            let amount = NonNegativeAmount::from_u64(receiver.1 as u64)
                .map_err(RawToTransactionRequestError::Amount)?;

            let memo = match receiver.2 {
                None => None,
                Some(memo_string) => Some(
                    MemoBytes::from_bytes(memo_string.as_bytes())
                        .map_err(RawToTransactionRequestError::Memo)?,
                ),
            };
            payments.push(Payment {
                recipient_address,
                amount,
                memo,
                label: None,
                message: None,
                other_params: vec![],
            });
        }

        TransactionRequest::new(payments).map_err(RawToTransactionRequestError::Zip321)
    }
}

type GISKit = GreedyInputSelector<
    TxMapAndMaybeTrees,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum DoProposeError {
    /// error in parsed addresses
    #[error("{0}")]
    Receiver(zcash_client_backend::zip321::Zip321Error),
    /// error in using trait to create proposal
    #[error("{:?}", {0})]
    Proposal(
        zcash_client_backend::data_api::error::Error<
            TxMapAndMaybeTreesTraitError,
            TxMapAndMaybeTreesTraitError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError<
                zcash_primitives::transaction::fees::zip317::FeeError,
                zcash_client_backend::wallet::NoteId,
            >,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("{0:?}")]
    ShieldProposal(
        zcash_client_backend::data_api::error::Error<
            TxMapAndMaybeTreesTraitError,
            TxMapAndMaybeTreesTraitError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError<
                zcash_primitives::transaction::fees::zip317::FeeError,
                Infallible,
            >,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
}

impl LightClient {
    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    pub async fn do_propose_spend(
        &self,
        request: TransactionRequest,
    ) -> Result<crate::data::proposal::TransferProposal, DoProposeError> {
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
        ); // review consider change strategy!

        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );

        let mut tmamt = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;

        let proposal = zcash_client_backend::data_api::wallet::propose_transfer::<
            TxMapAndMaybeTrees,
            ChainType,
            GISKit,
            TxMapAndMaybeTreesTraitError,
        >(
            tmamt.deref_mut(),
            &self.wallet.transaction_context.config.chain,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            NonZeroU32::MIN, //review! use custom constant?
        )
        .map_err(DoProposeError::Proposal)?;

        let mut latest_proposal_lock = self.latest_proposal.write().await;
        *latest_proposal_lock = Some(crate::data::proposal::ZingoProposal::Transfer(
            proposal.clone(),
        ));
        Ok(proposal)
    }

    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    pub async fn do_propose_shield(
        &self,
        _address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<crate::data::proposal::ShieldProposal, DoProposeError> {
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
        ); // review consider change strategy!

        let secp = secp256k1::Secp256k1::new();
        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );

        let mut tmamt = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            TxMapAndMaybeTrees,
            ChainType,
            GISKit,
            TxMapAndMaybeTreesTraitError,
        >(
            &mut tmamt,
            &self.wallet.transaction_context.config.chain,
            &input_selector,
            //review! how much?? configurable?
            NonNegativeAmount::const_from_u64(10_000),
            &self
                .wallet
                .wallet_capability()
                .transparent_child_keys()
                .expect("review! fix this expect")
                .iter()
                .map(|(_index, sk)| pubkey_to_address(&sk.public_key(&secp)))
                .collect::<Vec<_>>(),
            // review! do we want to require confirmations?
            // make it configurable?
            0,
        )
        .map_err(DoProposeError::ShieldProposal)?;

        //        *self.latest_proposal = Some(proposed_shield.clone());
        Ok(proposed_shield)
    }
}
