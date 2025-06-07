//! TODO: Add Mod Description Here!

use crate::wallet::send::SendProgress;

use super::LightClient;

impl LightClient {
    /// Wrapper for [`crate::wallet::LightWallet::send_progress`].
    pub async fn send_progress(&self) -> SendProgress {
        self.wallet.read().await.send_progress.clone()
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
            sending_account: zip32::AccountId,
        ) -> Result<NonEmpty<TxId>, SendError> {
            let mut wallet = self.wallet.write().await;
            let calculated_txids = wallet
                .calculate_transactions(proposal, sending_account)
                .await
                .map_err(SendError::CalculateSendError)?;
            self.latest_proposal = None;

            Ok(wallet
                .transmit_transactions(self.server_uri(), calculated_txids)
                .await?)
        }

        async fn shield(
            &mut self,
            proposal: &Proposal<zip317::FeeRule, Infallible>,
            shielding_account: zip32::AccountId,
        ) -> Result<NonEmpty<TxId>, SendError> {
            let mut wallet = self.wallet.write().await;
            let calculated_txids = wallet
                .calculate_transactions(proposal, shielding_account)
                .await
                .map_err(SendError::CalculateShieldError)?;
            self.latest_proposal = None;

            Ok(wallet
                .transmit_transactions(self.server_uri(), calculated_txids)
                .await?)
        }

        /// Re-transmits a previously calculated transaction that failed to send.
        pub async fn resend(&self, txid: TxId) -> Result<(), TransmissionError> {
            self.wallet
                .write()
                .await
                .transmit_transactions(self.server_uri(), NonEmpty::singleton(txid))
                .await?;

            Ok(())
        }

        /// Creates and transmits transactions from a stored proposal.
        pub async fn send_stored_proposal(&mut self) -> Result<NonEmpty<TxId>, SendError> {
            if let Some(proposal) = self.latest_proposal.clone() {
                match proposal {
                    ZingoProposal::Send {
                        proposal,
                        sending_account,
                    } => self.send(&proposal, sending_account).await,
                    ZingoProposal::Shield {
                        proposal,
                        shielding_account,
                    } => self.shield(&proposal, shielding_account).await,
                }
            } else {
                Err(SendError::NoStoredProposal)
            }
        }

        /// Proposes and transmits transactions from a transaction request skipping proposal confirmation.
        pub async fn quick_send(
            &mut self,
            request: TransactionRequest,
            account_id: zip32::AccountId,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self
                .wallet
                .write()
                .await
                .create_send_proposal(request, account_id)
                .await?;

            Ok(self.send(&proposal, account_id).await?)
        }

        /// Shields all transparent funds skipping proposal confirmation.
        pub async fn quick_shield(
            &mut self,
            account_id: zip32::AccountId,
        ) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self
                .wallet
                .write()
                .await
                .create_shield_proposal(account_id)
                .await?;

            Ok(self.shield(&proposal, account_id).await?)
        }
    }

    #[cfg(test)]
    mod test {
        //! all tests below (and in this mod) use example wallets, which describe real-world chains.

        use bip0039::Mnemonic;
        use pepper_sync::sync::SyncConfig;

        use crate::{
            lightclient::sync::test::sync_example_wallet,
            testutils::chain_generics::{
                conduct_chain::ConductChain as _, live_chain::LiveChain, with_assertions,
            },
            wallet::{LightWallet, WalletBase, WalletSettings, disk::testing::examples},
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
                    WalletBase::Mnemonic {
                        mnemonic: Mnemonic::from_phrase(ABANDON_ART_SEED.to_string()).unwrap(),
                        no_of_accounts: 1.try_into().unwrap(),
                    },
                    1.into(),
                    WalletSettings {
                        sync_config: SyncConfig {
                            transparent_address_discovery:
                                pepper_sync::sync::TransparentAddressDiscovery::minimal(),
                        },
                    },
                )
                .unwrap(),
                config,
                true,
            )
            .unwrap();
            let proposal = ProposalBuilder::default().build();
            lc.send(&proposal, zip32::AccountId::ZERO)
                .await
                .unwrap_err();
            // TODO: match on specific error
        }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send TESTNET: these assume the wallet has on-chain TAZ.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod testnet {
            use zcash_protocol::{PoolType, ShieldedProtocol};

            use crate::testutils::lightclient::get_base_address;

            use super::*;

            /// requires 1 confirmation: expect 3 minute runtime
            #[ignore = "live testnet: testnet relies on NU6"]
            #[tokio::test]
            async fn glory_goddess_simple_send() {
                let case = examples::NetworkSeedVersion::Testnet(
                    examples::TestnetSeedVersion::GloryGoddess,
                );
                let mut client = sync_example_wallet(case).await;

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &mut client,
                    true,
                )
                .await
                .unwrap();
            }

            #[ignore = "live testnet: testnet relies on NU6"]
            #[tokio::test]
            /// this is a live sync test. its execution time scales linearly since last updated
            /// this is a live send test. whether it can work depends on the state of live wallet on the blockchain
            /// note: live send waits 2 minutes for confirmation. expect 3min runtime
            async fn testnet_send_to_self_orchard() {
                let case = examples::NetworkSeedVersion::Testnet(
                    examples::TestnetSeedVersion::ChimneyBetter(
                        examples::ChimneyBetterVersion::Latest,
                    ),
                );

                let mut client = sync_example_wallet(case).await;
                let client_addr =
                    get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &mut client,
                    vec![(&client_addr, 10_000, None)],
                    vec![],
                    false,
                )
                .await
                .unwrap();
            }

            #[ignore = "live testnet: testnet relies on NU6"]
            #[tokio::test]
            /// this is a live sync test. its execution time scales linearly since last updated
            /// note: live send waits 2 minutes for confirmation. expect 3min runtime
            async fn testnet_shield() {
                let case = examples::NetworkSeedVersion::Testnet(
                    examples::TestnetSeedVersion::ChimneyBetter(
                        examples::ChimneyBetterVersion::Latest,
                    ),
                );

                let mut client = sync_example_wallet(case).await;

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &mut client,
                    true,
                )
                .await
                .unwrap();
            }
        }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send MAINNET: spends on-chain ZEC.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod mainnet {
            use zcash_protocol::{PoolType, ShieldedProtocol};

            use crate::testutils::lightclient::get_base_address;

            use super::*;

            /// requires 1 confirmation: expect 3 minute runtime
            #[tokio::test]
            #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            async fn mainnet_send_to_self_orchard() {
                let case = examples::NetworkSeedVersion::Mainnet(
                    examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
                );

                let mut client = sync_example_wallet(case).await;
                let client_addr =
                    get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &mut client,
                    vec![(&client_addr, 10_000, None)],
                    vec![],
                    false,
                )
                .await
                .unwrap();
            }

            /// requires 1 confirmation: expect 3 minute runtime
            #[tokio::test]
            #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            async fn mainnet_send_to_self_sapling() {
                let case = examples::NetworkSeedVersion::Mainnet(
                    examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
                );

                let mut client = sync_example_wallet(case).await;
                let client_addr =
                    get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Sapling)).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &mut client,
                    vec![(&client_addr, 400_000, None)],
                    vec![],
                    false,
                )
                .await
                .unwrap();
            }

            /// requires 2 confirmations: expect 6 minute runtime
            #[tokio::test]
            #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            async fn mainnet_send_to_self_transparent_and_then_shield() {
                let case = examples::NetworkSeedVersion::Mainnet(
                    examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
                );

                let mut client = sync_example_wallet(case).await;
                let client_addr = get_base_address(&client, PoolType::Transparent).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &mut client,
                    vec![(&client_addr, 400_000, None)],
                    vec![],
                    false,
                )
                .await
                .unwrap();

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &mut client,
                    false,
                )
                .await
                .unwrap();
            }
        }
    }
}
