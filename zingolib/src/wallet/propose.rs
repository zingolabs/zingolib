//! creating proposals from wallet data

use std::num::NonZeroU32;

use pepper_sync::keys::transparent::TransparentScope;
use zcash_client_backend::{
    data_api::wallet::input_selection::GreedyInputSelector,
    fees::{DustAction, DustOutputPolicy},
    zip321::TransactionRequest,
};
use zcash_protocol::{ShieldedProtocol, value::Zatoshis};

use crate::config::ChainType;

use super::{
    LightWallet,
    error::{ProposeSendError, ProposeShieldError, WalletError},
    send::change_memo_from_transaction_request,
};

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
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            Some(memo),
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AllowDustChange, None),
        );
        let network = self.network;

        zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &network,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            &change_strategy,
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
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedProtocol::Orchard,
            DustOutputPolicy::new(DustAction::AllowDustChange, None),
        );
        let network = self.network;

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &network,
            &input_selector,
            &change_strategy,
            Zatoshis::const_from_u64(10_000),
            &self.get_transparent_addresses()?,
            zip32::AccountId::ZERO,
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
    use zcash_protocol::{PoolType, ShieldedProtocol};

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

        let pool = PoolType::Shielded(ShieldedProtocol::Orchard);
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
