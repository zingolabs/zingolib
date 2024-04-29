//! LightClient function do_propose generates a proposal to send to specified addresses.

use log::debug;

use zcash_client_backend::{
    address::Address,
    zip321::{Payment, TransactionRequest},
};
use zcash_primitives::{
    consensus::BlockHeight,
    memo::MemoBytes,
    transaction::{components::amount::NonNegativeAmount, fees::zip317::MINIMUM_FEE},
};
use zcash_proofs::prover::LocalTxProver;

use crate::{utils::zatoshis_from_u64, wallet::Pool};
use {
    crate::{error::ZingoLibError, wallet::tx_map_and_maybe_trees::TxMapAndMaybeTrees},
    std::{num::NonZeroU32, ops::DerefMut},
    zcash_client_backend::{
        data_api::wallet::input_selection::GreedyInputSelector, ShieldedProtocol,
    },
    zcash_primitives::transaction::TxId,
    zingoconfig::ChainType,
};

use super::{LightClient, LightWalletSendProgress};

type GISKit = GreedyInputSelector<
    TxMapAndMaybeTrees,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

/// converts from raw receivers to TransactionRequest
pub fn receivers_becomes_transaction_request(
    receivers: Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>,
) -> Result<TransactionRequest, zcash_client_backend::zip321::Zip321Error> {
    let mut payments = vec![];
    for out in receivers.clone() {
        payments.push(Payment {
            recipient_address: out.0,
            amount: out.1,
            memo: out.2,
            label: None,
            message: None,
            other_params: vec![],
        });
    }

    TransactionRequest::new(payments)
}

use thiserror::Error;

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum DoProposeError {
    #[error("{0}")]
    Receiver(zcash_client_backend::zip321::Zip321Error),
    #[error("{0}")]
    Proposal(crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesError),
}

impl super::LightClient {
    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    pub async fn do_propose_spend(
        &self,
        receivers: Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>,
    ) -> Result<crate::data::proposal::TransferProposal, String> {
        let request =
            receivers_becomes_transaction_request(receivers).map_err(|e| e.to_string())?;

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
            ZingoLibError,
        >(
            tmamt.deref_mut(),
            &self.wallet.transaction_context.config.chain,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            NonZeroU32::MIN, //review! use custom constant?
        )
        .map_err(|e| ZingoLibError::Error(format!("error this function todo error {:?}", e)))?;

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
    ) -> Result<crate::data::proposal::ShieldProposal, String> {
        todo!()
    }

    /// Unstable function to expose the zip317 interface for development
    // TODO: add correct functionality and doc comments / tests
    pub async fn do_send_proposal(&self) -> Result<Vec<TxId>, String> {
        if let Some(proposal) = self.latest_proposal.read().await.as_ref() {
            match proposal {
                crate::lightclient::ZingoProposal::Transfer(_) => {
                    Ok(vec![TxId::from_bytes([1u8; 32])])
                }
                crate::lightclient::ZingoProposal::Shield(_) => {
                    Ok(vec![TxId::from_bytes([222u8; 32])])
                }
            }
        } else {
            Err("No proposal. Call do_propose first.".to_string())
        }
    }
}
