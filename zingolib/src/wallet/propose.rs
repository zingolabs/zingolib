//! creating proposals from wallet data

use std::{convert::Infallible, num::NonZeroU32, ops::DerefMut as _};

use thiserror::Error;
use zcash_client_backend::{
    data_api::wallet::input_selection::GreedyInputSelector,
    zip321::{TransactionRequest, Zip321Error},
    ShieldedProtocol,
};
use zcash_primitives::{memo::MemoBytes, transaction::components::amount::NonNegativeAmount};

use crate::config::ChainType;

use super::{
    send::change_memo_from_transaction_request,
    tx_map::{TxMap, TxMapTraitError},
    LightWallet,
};

type GISKit =
    GreedyInputSelector<TxMap, zcash_client_backend::fees::zip317::SingleOutputChangeStrategy>;

// This private helper is a very small DRY, but it has already corrected a minor
// divergence in change strategy.
//  Because shielding operations are never expected to create dust notes this change
// is not a bugfix.
fn build_default_giskit(memo: Option<MemoBytes>) -> GISKit {
    let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
        zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
        memo,
        ShieldedProtocol::Orchard,
    ); // review consider change strategy!

    GISKit::new(
        change_strategy,
        zcash_client_backend::fees::DustOutputPolicy::new(
            zcash_client_backend::fees::DustAction::AllowDustChange,
            None,
        ),
    )
}

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum ProposeSendError {
    /// error in using trait to create spend proposal
    #[error("{0}")]
    Proposal(
        zcash_client_backend::data_api::error::Error<
            TxMapTraitError,
            TxMapTraitError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError<
                zcash_primitives::transaction::fees::zip317::FeeError,
                zcash_client_backend::wallet::NoteId,
            >,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    /// failed to construct a transaction request
    #[error("{0}")]
    TransactionRequestFailed(#[from] Zip321Error),
    /// send all is transferring no value
    #[error("send all is transferring no value. only enough funds to pay the fees!")]
    ZeroValueSendAll,
    /// failed to calculate balance.
    #[error("failed to calculated balance. {0}")]
    BalanceError(#[from] crate::wallet::error::BalanceError),
}

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum ProposeShieldError {
    /// error in parsed addresses
    #[error("{0}")]
    Receiver(zcash_client_backend::zip321::Zip321Error),
    #[error("{0}")]
    /// error in using trait to create shielding proposal
    Component(
        zcash_client_backend::data_api::error::Error<
            TxMapTraitError,
            TxMapTraitError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError<
                zcash_primitives::transaction::fees::zip317::FeeError,
                Infallible,
            >,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
}

impl LightWallet {
    /// Creates a proposal from a transaction request.
    pub(crate) async fn create_send_proposal(
        &self,
        request: TransactionRequest,
    ) -> Result<crate::data::proposal::ProportionalFeeProposal, ProposeSendError> {
        let memo = change_memo_from_transaction_request(&request);

        let input_selector = build_default_giskit(Some(memo));
        let mut tmamt = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;

        zcash_client_backend::data_api::wallet::propose_transfer::<
            TxMap,
            ChainType,
            GISKit,
            TxMapTraitError,
        >(
            tmamt.deref_mut(),
            &self.transaction_context.config.chain,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            NonZeroU32::MIN, //review! use custom constant?
        )
        .map_err(ProposeSendError::Proposal)
    }

    /// The shield operation consumes a proposal that transfers value
    /// into the Orchard pool.
    ///
    /// The proposal is generated with this method, which operates on
    /// the balance transparent pool, without other input.
    /// In other words, shield does not take a user-specified amount
    /// to shield, rather it consumes all transparent value in the wallet that
    /// can be consumsed without costing more in zip317 fees than is being transferred.
    pub(crate) async fn create_shield_proposal(
        &self,
    ) -> Result<crate::data::proposal::ProportionalFeeShieldProposal, ProposeShieldError> {
        let input_selector = build_default_giskit(None);

        let mut tmamt = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            TxMap,
            ChainType,
            GISKit,
            TxMapTraitError,
        >(
            &mut tmamt,
            &self.transaction_context.config.chain,
            &input_selector,
            // don't shield dust
            NonNegativeAmount::const_from_u64(10_000),
            &self.get_transparent_addresses(),
            // review! do we want to require confirmations?
            // make it configurable?
            0,
        )
        .map_err(ProposeShieldError::Component)?;

        Ok(proposed_shield)
    }
}
