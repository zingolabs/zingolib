//! TODO: Add Mod Description Here!

use crate::wallet::send::SendProgress;

use super::LightClient;

impl LightClient {
    /// Wrapper for [`crate::wallet::LightWallet::send_progress`].
    pub async fn send_progress(&self) -> SendProgress {
        self.wallet.lock().await.send_progress.clone()
    }
}

/// patterns for newfangled propose flow
pub mod send_with_proposal {
    use std::convert::Infallible;

    use nonempty::NonEmpty;

    use zcash_client_backend::proposal::Proposal;
    use zcash_client_backend::zip321::TransactionRequest;

    use zcash_primitives::transaction::TxId;
    use zcash_primitives::transaction::fees::zip317;

    use crate::data::proposal::ZingoProposal;
    use crate::lightclient::LightClient;
    use crate::lightclient::error::{QuickSendError, QuickShieldError, SendError};
    use crate::wallet::error::TransmissionError;
    use crate::wallet::output::OutputRef;

    impl LightClient {
        async fn send(
            &mut self,
            proposal: &Proposal<zip317::FeeRule, OutputRef>,
        ) -> Result<NonEmpty<TxId>, SendError> {
            let mut wallet = self.wallet.lock().await;
            let calculated_txids = wallet
                .calculate_transactions(proposal)
                .await
                .map_err(SendError::CalculateSendError)?;
            self.latest_proposal = None;

            Ok(wallet
                .transmit_transactions(self.get_server_uri(), calculated_txids)
                .await?)
        }

        async fn shield(
            &mut self,
            proposal: &Proposal<zip317::FeeRule, Infallible>,
        ) -> Result<NonEmpty<TxId>, SendError> {
            let mut wallet = self.wallet.lock().await;
            let calculated_txids = wallet
                .calculate_transactions(proposal)
                .await
                .map_err(SendError::CalculateShieldError)?;
            self.latest_proposal = None;

            Ok(wallet
                .transmit_transactions(self.get_server_uri(), calculated_txids)
                .await?)
        }

        /// Re-transmits a previously calculated transaction that failed to send.
        pub async fn resend(&self, txid: TxId) -> Result<(), TransmissionError> {
            self.wallet
                .lock()
                .await
                .transmit_transactions(self.get_server_uri(), NonEmpty::singleton(txid))
                .await?;

            Ok(())
        }

        /// Calculates, signs and broadcasts transactions from a stored proposal.
        pub async fn send_stored_proposal(&mut self) -> Result<NonEmpty<TxId>, SendError> {
            if let Some(proposal) = self.latest_proposal.clone() {
                match proposal {
                    ZingoProposal::Transfer(transfer_proposal) => {
                        self.send(&transfer_proposal).await
                    }
                    ZingoProposal::Shield(shield_proposal) => self.shield(&shield_proposal).await,
                }
            } else {
                Err(SendError::NoStoredProposal)
            }
        }

        /// Proposes and transmits transactions from a transaction request skipping proposal confirmation.
        pub async fn quick_send(
            &mut self,
            request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self
                .wallet
                .lock()
                .await
                .create_send_proposal(request)
                .await?;

            Ok(self.send(&proposal).await?)
        }

        /// Shields all transparent funds skipping proposal confirmation.
        pub async fn quick_shield(&mut self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.wallet.lock().await.create_shield_proposal().await?;

            Ok(self.shield(&proposal).await?)
        }
    }

    #[cfg(test)]
    mod test {
        //! all tests below (and in this mod) use example wallets, which describe real-world chains.

        use crate::{
            lightclient::sync::test::sync_example_wallet,
            testutils::chain_generics::{
                conduct_chain::ConductChain as _, networked::NetworkedTestEnvironment,
                with_assertions,
            },
            wallet::{LightWallet, WalletBase, disk::testing::examples},
        };

        #[tokio::test]
        async fn complete_and_broadcast_unconnected_error() {
            use crate::{
                config::ZingoConfigBuilder, lightclient::LightClient,
                mocks::proposal::ProposalBuilder,
            };
            use testvectors::seeds::ABANDON_ART_SEED;

            let config = ZingoConfigBuilder::default().create();
            let mut lc = LightClient::create_from_wallet(
                LightWallet::new(
                    config.chain,
                    WalletBase::MnemonicPhrase(ABANDON_ART_SEED.to_string()),
                    1.into(),
                )
                .unwrap(),
                config,
                true,
            )
            .unwrap();
            let proposal = ProposalBuilder::default().build();
            lc.send(&proposal).await.unwrap_err();
            // TODO: match on specific error
        }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send TESTNET: these assume the wallet has on-chain TAZ.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod testnet {
            use zcash_protocol::{PoolType, ShieldedProtocol};

            use crate::testutils::lightclient::get_base_address;

            use super::*;

            #[tokio::test]
            /// this is a networked sync test. its execution time scales linearly since last updated
            /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
            /// note: networked send waits 2 minutes for confirmation. expect 3min runtime
            async fn testnet_send_to_self_orchard_glory_goddess() {
                let case = examples::NetworkSeedVersion::Testnet(
                    examples::TestnetSeedVersion::GloryGoddess,
                );

                let mut client = sync_example_wallet(case).await;
                let client_addr =
                    get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut NetworkedTestEnvironment::setup().await,
                    &mut client,
                    vec![(&client_addr, 10_000, None)],
                    vec![],
                    false,
                )
                .await
                .unwrap();
            }
        }
    }
}
