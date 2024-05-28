//! LightClient function do_propose generates a proposal to send to specified addresses.

use std::convert::Infallible;
use std::num::NonZeroU32;
use std::ops::DerefMut;

use zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelector;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_client_backend::zip321::Zip321Error;
use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use thiserror::Error;

use crate::data::proposal::ShieldProposal;
use crate::data::proposal::TransferProposal;
use crate::data::proposal::ZingoProposal;
use crate::lightclient::LightClient;
use crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTrees;
use crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError;
use zingoconfig::ChainType;

type GISKit = GreedyInputSelector<
    TxMapAndMaybeTrees,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum ProposeSendError {
    #[error("{0:?}")]
    /// error in using trait to create spend proposal
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
    /// failed to construct a transaction request
    TransactionRequestFailed(Zip321Error),
}

/// Errors that can result from do_propose
#[derive(Debug, Error)]
pub enum ProposeShieldError {
    /// error in parsed addresses
    #[error("{0:?}")]
    Receiver(zcash_client_backend::zip321::Zip321Error),
    #[error("{0:?}")]
    /// error in using trait to create shielding proposal
    Component(
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
    /// Stores a proposal in the `latest_proposal` field of the LightClient.
    /// This field must be populated in order to then send a transaction.
    async fn store_proposal(&self, proposal: ZingoProposal) {
        let mut latest_proposal_lock = self.latest_proposal.write().await;
        *latest_proposal_lock = Some(proposal);
    }

    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    pub async fn create_send_proposal(
        &self,
        request: TransactionRequest,
    ) -> Result<TransferProposal, ProposeSendError> {
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

        zcash_client_backend::data_api::wallet::propose_transfer::<
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
        .map_err(ProposeSendError::Proposal)
    }

    /// Unstable function to expose the zip317 interface for development
    pub async fn propose_send(
        &self,
        request: TransactionRequest,
    ) -> Result<TransferProposal, ProposeSendError> {
        let proposal = self.create_send_proposal(request).await?;
        self.store_proposal(ZingoProposal::Transfer(proposal.clone()))
            .await;
        Ok(proposal)
    }

    /*
    /// Unstable function to expose the zip317 interface for development
    // TOdo: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    #[cfg(test)]
    pub async fn propose_send_all(
        &self,
        _address: zcash_keys::address::Address,
        _memo: Option<zcash_primitives::memo::MemoBytes>,
    ) -> Result<crate::data::proposal::TransferProposal, String> {
        use crate::mocks::ProposalBuilder;

        let proposal = ProposalBuilder::default().build();
        self.store_proposal(ZingoProposal::Transfer(proposal.clone()))
            .await;
        Ok(proposal)
    }
    */

    fn get_transparent_addresses(&self) -> Vec<zcash_primitives::legacy::TransparentAddress> {
        self.wallet
            .wallet_capability()
            .transparent_child_addresses()
            .iter()
            .map(|(_index, sk)| *sk)
            .collect::<Vec<_>>()
    }

    /// The shield operation consumes a proposal that transfers value
    /// into the Orchard pool.
    ///
    /// The proposal is generated with this method, which operates on
    /// the balances in the wallet pools, without other input.
    /// In other words, shield does not take a user-specified amount
    /// to shield, rather it consumes all transparent value in the wallet that
    /// can be consumsed without costing more in zip317 fees than is being transferred.
    pub(crate) async fn create_shield_proposal(
        &self,
    ) -> Result<crate::data::proposal::ShieldProposal, ProposeShieldError> {
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
        ); // review consider change strategy!

        let input_selector = GISKit::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::new(
                zcash_client_backend::fees::DustAction::AllowDustChange,
                None,
            ),
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
            &self.get_transparent_addresses(),
            // review! do we want to require confirmations?
            // make it configurable?
            0,
        )
        .map_err(ProposeShieldError::Component)?;

        Ok(proposed_shield)
    }

    /// Unstable function to expose the zip317 interface for development
    pub async fn propose_shield(&self) -> Result<ShieldProposal, ProposeShieldError> {
        let proposal = self.create_shield_proposal().await?;
        self.store_proposal(ZingoProposal::Shield(proposal.clone()))
            .await;
        Ok(proposal)
    }
}

#[cfg(test)]
mod shielding {
    use crate::lightclient::propose::ProposeShieldError;

    async fn create_basic_client() -> crate::lightclient::LightClient {
        crate::lightclient::LightClient::create_unconnected(
            &zingoconfig::ZingoConfigBuilder::default().create(),
            crate::wallet::WalletBase::MnemonicPhrase(
                zingo_testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            ),
            0,
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn propose_shield_missing_scan_prerequisite() {
        let basic_client = create_basic_client().await;
        let propose_shield_result = basic_client.create_shield_proposal().await;
        match propose_shield_result {
            Err(ProposeShieldError::Component(
                zcash_client_backend::data_api::error::Error::ScanRequired,
            )) => true,
            _ => panic!("Unexpected error state!"),
        };
    }
    #[tokio::test]
    async fn get_transparent_addresses() {
        let basic_client = create_basic_client().await;
        assert_eq!(
            basic_client.get_transparent_addresses(),
            [zcash_primitives::legacy::TransparentAddress::PublicKeyHash(
                [
                    161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148,
                    255, 189, 198, 33
                ]
            )]
        );
    }
}
