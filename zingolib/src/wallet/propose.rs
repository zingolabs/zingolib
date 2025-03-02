//! creating proposals from wallet data

use std::{convert::Infallible, num::NonZeroU32};

use pepper_sync::keys::transparent::TransparentScope;
use zcash_client_backend::{
    data_api::wallet::input_selection::GreedyInputSelector,
    zip321::{TransactionRequest, Zip321Error},
    ShieldedProtocol,
};
use zcash_primitives::{memo::MemoBytes, transaction::components::amount::NonNegativeAmount};

use crate::config::ChainType;

use super::{error::WalletError, send::change_memo_from_transaction_request, LightWallet};

type GISKit = GreedyInputSelector<
    LightWallet,
    zcash_client_backend::fees::zip317::SingleOutputChangeStrategy,
>;

fn build_default_giskit(memo: Option<MemoBytes>) -> GISKit {
    let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
        zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
        memo,
        ShieldedProtocol::Orchard,
    );

    GISKit::new(
        change_strategy,
        zcash_client_backend::fees::DustOutputPolicy::new(
            zcash_client_backend::fees::DustAction::AllowDustChange,
            None,
        ),
    )
}

/// Errors that can result from do_propose
#[derive(Debug, thiserror::Error)]
pub enum ProposeSendError {
    /// error in using trait to create spend proposal
    #[error("{0}")]
    Proposal(
        zcash_client_backend::data_api::error::Error<
            WalletError,
            WalletError,
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
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum ProposeShieldError {
    /// error in parsed addresses
    #[error("{0}")]
    Receiver(zcash_client_backend::zip321::Zip321Error),
    /// error in using trait to create shielding proposal
    #[error("{0}")]
    Component(
        zcash_client_backend::data_api::error::Error<
            WalletError,
            WalletError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError<
                zcash_primitives::transaction::fees::zip317::FeeError,
                Infallible,
            >,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Not enough transparent funds to shield.")]
    Insufficient,
}

impl LightWallet {
    /// Creates a proposal from a transaction request.
    pub(crate) async fn create_send_proposal(
        &mut self,
        request: TransactionRequest,
    ) -> Result<crate::data::proposal::ProportionalFeeProposal, ProposeSendError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        let memo = change_memo_from_transaction_request(&request, refund_address_count);
        let input_selector = build_default_giskit(Some(memo));

        let network = self.network;

        zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GISKit,
            WalletError,
        >(
            self,
            &network,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            NonZeroU32::MIN,
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
    /// can be consumed without costing more in zip317 fees than is being transferred.
    pub(crate) async fn create_shield_proposal(
        &mut self,
    ) -> Result<crate::data::proposal::ProportionalFeeShieldProposal, ProposeShieldError> {
        let network = self.network;
        let input_selector = build_default_giskit(None);

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            LightWallet,
            ChainType,
            GISKit,
            WalletError,
        >(
            self,
            &network,
            &input_selector,
            NonNegativeAmount::const_from_u64(10_000),
            &self.get_transparent_addresses(),
            1,
        )
        .map_err(ProposeShieldError::Component)?;

        for step in proposed_shield.steps().iter() {
            if step
                .balance()
                .proposed_change()
                .iter()
                .fold(0, |total_out, output| total_out + output.value().into_u64())
                == 0
            {
                return Err(ProposeShieldError::Insufficient);
            }
        }

        Ok(proposed_shield)
    }
}

#[cfg(test)]
mod test {
    use zcash_client_backend::PoolType;

    use crate::{
        testutils::lightclient::from_inputs::transaction_request_from_send_inputs,
        wallet::disk::testing::examples,
    };

    /// this test loads an example wallet with existing sapling finds
    #[ignore = "for some reason this is does not work without network, even though it should be possible"]
    #[tokio::test]
    async fn example_mainnet_hhcclaltpcckcsslpcnetblr_80b5594ac_propose_100_000_to_self() {
        let client = examples::NetworkSeedVersion::Mainnet(
            examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
        )
        .load_example_wallet_with_client()
        .await;
        let mut wallet = client.wallet.lock().await;

        let pool = PoolType::Shielded(zcash_client_backend::ShieldedProtocol::Orchard);
        let self_address = wallet.get_first_address(pool).unwrap();

        let receivers = vec![(self_address.as_str(), 100_000, None)];
        let request = transaction_request_from_send_inputs(receivers)
            .expect("actually all of this logic oughta be internal to propose");

        wallet
            .create_send_proposal(request)
            .await
            .expect("can propose from existing data");
    }
}
