//! LightClient function do_propose generates a proposal to send to specified addresses.

use std::convert::Infallible;
use std::num::NonZeroU32;
use std::ops::DerefMut;

use zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelector;
use zcash_client_backend::zip321::Payment;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_client_backend::zip321::Zip321Error;
use zcash_client_backend::ShieldedProtocol;
use zcash_keys::address::Address;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::components::amount::BalanceError;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use thiserror::Error;

use crate::{
    data::proposal::ShieldProposal, wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError,
};
use crate::{data::proposal::TransferProposal, wallet::tx_map_and_maybe_trees::TxMapAndMaybeTrees};
use crate::{data::proposal::ZingoProposal, lightclient::LightClient};
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
    /// error in using trait to create spend proposal
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
    /// error in using trait to create shielding proposal
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
    async fn store_proposal(&self, proposal: ZingoProposal) {
        let mut latest_proposal_lock = self.latest_proposal.write().await;
        *latest_proposal_lock = Some(proposal);
    }
    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    pub(crate) async fn do_propose_send(
        &self,
        request: TransactionRequest,
    ) -> Result<TransferProposal, DoProposeError> {
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

        Ok(proposal)
    }

    /// Unstable function to expose the zip317 interface for development
    pub async fn do_propose_send_and_remember(
        &self,
        request: TransactionRequest,
    ) -> Result<TransferProposal, DoProposeError> {
        let proposal = self.do_propose_send(request).await?;
        self.store_proposal(ZingoProposal::Transfer(proposal.clone()))
            .await;
        Ok(proposal)
    }

    fn get_transparent_addresses(
        &self,
    ) -> Result<Vec<zcash_primitives::legacy::TransparentAddress>, DoProposeError> {
        let secp = secp256k1::Secp256k1::new();
        Ok(self
            .wallet
            .wallet_capability()
            .transparent_child_keys()
            .map_err(|_e| {
                DoProposeError::ShieldProposal(
                    zcash_client_backend::data_api::error::Error::DataSource(
                        TxMapAndMaybeTreesTraitError::NoSpendCapability,
                    ),
                )
            })?
            .iter()
            .map(|(_index, sk)| {
                #[allow(deprecated)]
                zcash_primitives::legacy::keys::pubkey_to_address(&sk.public_key(&secp))
            })
            .collect::<Vec<_>>())
    }
    /// The shield operation consumes a proposal that transfers value
    /// into the Orchard pool.
    ///
    /// The proposal is generated with this method, which operates on
    /// the balances in the wallet pools, without other input.
    /// In other words, shield does not take a user-specified amount
    /// to shield, rather it consumes all transparent value in the wallet that
    /// can be consumsed without costing more in zip317 fees than is being transferred.
    pub(crate) async fn do_propose_shield(
        &self,
    ) -> Result<crate::data::proposal::ShieldProposal, DoProposeError> {
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

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            TxMapAndMaybeTrees,
            ChainType,
            GISKit,
            TxMapAndMaybeTreesTraitError,
        >(
            &mut tmamt,
            &self.wallet.transaction_context.config.chain,
            &input_selector,
            // don't shield dust
            NonNegativeAmount::const_from_u64(10_000),
            &self.get_transparent_addresses()?,
            // review! do we want to require confirmations?
            // make it configurable?
            0,
        )
        .map_err(DoProposeError::ShieldProposal)?;

        Ok(proposed_shield)
    }

    /// Unstable function to expose the zip317 interface for development
    pub async fn do_propose_shield_and_remember(&self) -> Result<ShieldProposal, DoProposeError> {
        let proposal = self.do_propose_shield().await?;
        self.store_proposal(ZingoProposal::Shield(proposal.clone()))
            .await;
        Ok(proposal)
    }
}
#[cfg(test)]
mod shielding {
    #[tokio::test]
    async fn get_transparent_addresses() {
        let client = crate::lightclient::LightClient::create_unconnected(
            &zingoconfig::ZingoConfigBuilder::default().create(),
            crate::wallet::WalletBase::MnemonicPhrase(
                zingo_testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            ),
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            client.get_transparent_addresses().unwrap(),
            [zcash_primitives::legacy::TransparentAddress::PublicKeyHash(
                [
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ]
            )]
        );
    }
}
