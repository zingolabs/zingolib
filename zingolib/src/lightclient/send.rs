//! TODO: Add Mod Description Here!

use std::convert::Infallible;

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::transaction::{TxId, fees::zip317};

use zingo_status::confirmation_status::ConfirmationStatus;

use crate::data::proposal::ZingoProposal;
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError, TransmissionError};
use crate::wallet::error::WalletError;
use crate::wallet::output::OutputRef;

impl LightClient {
    async fn send(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let calculated_txids = self
            .wallet
            .write()
            .await
            .calculate_transactions(proposal, sending_account)
            .await
            .map_err(SendError::CalculateSendError)?;

        self.transmit_transactions(calculated_txids).await
    }

    async fn shield(
        &mut self,
        proposal: Proposal<zip317::FeeRule, Infallible>,
        shielding_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let calculated_txids = self
            .wallet
            .write()
            .await
            .calculate_transactions(proposal, shielding_account)
            .await
            .map_err(SendError::CalculateShieldError)?;

        self.transmit_transactions(calculated_txids).await
    }

    /// Creates and transmits transactions from a stored proposal.
    ///
    /// If sync was running prior to creating a send proposal, sync will have been paused. If `resume_sync` is `true`, sync will be resumed after sending the stored proposal.
    pub async fn send_stored_proposal(
        &mut self,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let opt_proposal = self.wallet.write().await.take_proposal();
        if let Some(proposal) = opt_proposal {
            let txids = match proposal {
                ZingoProposal::Send {
                    proposal,
                    sending_account,
                } => self.send(proposal, sending_account).await,
                ZingoProposal::Shield {
                    proposal,
                    shielding_account,
                } => self.shield(proposal, shielding_account).await,
            }?;

            if resume_sync {
                let _ignore_error = self.resume_sync();
            }

            Ok(txids)
        } else {
            Err(SendError::NoStoredProposal.into())
        }
    }

    /// Proposes and transmits transactions from a transaction request skipping proposal confirmation.
    ///
    /// If sync is running, sync will be paused before creating the send proposal. If `resume_sync` is `true`, sync will be resumed after send.
    pub async fn quick_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let _ignore_error = self.pause_sync();
        let proposal = self
            .wallet
            .write()
            .await
            .create_send_proposal(request, account_id)
            .map_err(SendError::ProposeSendError)?;
        let txids = self.send(proposal, account_id).await?;
        if resume_sync {
            let _ignore_error = self.resume_sync();
        }

        Ok(txids)
    }

    /// Shields all transparent funds skipping proposal confirmation.
    pub async fn quick_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let proposal = self
            .wallet
            .write()
            .await
            .create_shield_proposal(account_id)
            .map_err(SendError::ProposeShieldError)?;

        self.shield(proposal, account_id).await
    }

    /// Tranmits calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Returns list of txids for successfully transmitted transactions.
    async fn transmit_transactions(
        &mut self,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let mut wallet = self.wallet.write().await;
        for txid in calculated_txids.iter() {
            let calculated_transaction = wallet
                .wallet_transactions
                .get_mut(txid)
                .ok_or(WalletError::TransactionNotFound(*txid))?;
            let height = calculated_transaction.status().get_height();

            if !matches!(
                calculated_transaction.status(),
                ConfirmationStatus::Calculated(_)
            ) {
                return Err(SendError::TransmissionError(
                    TransmissionError::IncorrectTransactionStatus(*txid),
                )
                .into());
            }

            let mut transaction_bytes = vec![];
            calculated_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .map_err(|e| {
                    // TODO: set tx to failed
                    WalletError::TransactionWrite(e)
                })?;

            let txid_from_server = crate::grpc_connector::send_transaction(
                self.server_uri(),
                transaction_bytes.into_boxed_slice(),
            )
            .await
            .map_err(|e| {
                // TODO: set tx to failed
                SendError::TransmissionError(TransmissionError::TransmissionFailed(e))
            })?;
            calculated_transaction
                .update_status(ConfirmationStatus::Transmitted(height), crate::utils::now());
            wallet.save_required = true;

            let txid_from_server =
                crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())
                    .map_err(WalletError::ConversionFailed)?;
            if txid_from_server != *txid {
                // during darkside tests, the server may report a different txid to the one calculated.
                #[cfg(not(feature = "darkside_tests"))]
                {
                    return Err(
                        TransmissionError::IncorrectTxidFromServer(txid, txid_from_server).into(),
                    );
                }
            }
        }

        Ok(calculated_txids)
    }
}

#[cfg(test)]
mod test {
    //! all tests below (and in this mod) use example wallets, which describe real-world chains.

    use std::num::NonZeroU32;

    use bip0039::Mnemonic;
    use pepper_sync::config::SyncConfig;

    use crate::{
        lightclient::sync::test::sync_example_wallet,
        testutils::chain_generics::{
            conduct_chain::ConductChain as _, networked::NetworkedTestEnvironment, with_assertions,
        },
        wallet::{LightWallet, WalletBase, WalletSettings, disk::testing::examples},
    };

    #[tokio::test]
    async fn complete_and_broadcast_unconnected_error() {
        use crate::{
            config::ZingoConfigBuilder, lightclient::LightClient, mocks::proposal::ProposalBuilder,
        };
        use zingo_test_vectors::seeds::ABANDON_ART_SEED;

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
                            pepper_sync::config::TransparentAddressDiscovery::minimal(),
                        performance_level: pepper_sync::config::PerformanceLevel::High,
                    },
                    min_confirmations: NonZeroU32::try_from(1).unwrap(),
                },
            )
            .unwrap(),
            config,
            true,
        )
        .unwrap();
        let proposal = ProposalBuilder::default().build();
        lc.send(proposal, zip32::AccountId::ZERO).await.unwrap_err();
        // TODO: match on specific error
    }

    /// live sync: execution time increases linearly until example wallet is upgraded
    /// live send TESTNET: these assume the wallet has on-chain TAZ.
    /// waits up to five blocks for confirmation per transaction. see [`zingolib/src/testutils/chain_generics/live_chain.rs`]
    /// as of now, average block time is supposedly about 75 seconds
    mod testnet {
        use zcash_protocol::{PoolType, ShieldedProtocol};

        use crate::testutils::lightclient::get_base_address;

        use super::*;

        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_orchard_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_sapling_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Sapling)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        /// about 273 seconds
        async fn testnet_send_to_self_transparent_and_then_shield_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr = get_base_address(&client, PoolType::Transparent).await;

            let environment = &mut NetworkedTestEnvironment::setup().await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_001, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
        #[ignore = "this needs to pass CI, but we arent there with testnet"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_all_pools_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;
            let environment = &mut NetworkedTestEnvironment::setup().await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Orchard)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 14_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedProtocol::Sapling)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 15_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr = get_base_address(&client, PoolType::Transparent).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
    }
}
