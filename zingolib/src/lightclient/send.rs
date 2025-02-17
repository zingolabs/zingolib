//! TODO: Add Mod Description Here!

use super::LightClient;
use super::LightWalletSendProgress;

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.lock().await.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
        })
    }
}

/// patterns for newfangled propose flow
pub mod send_with_proposal {
    use std::convert::Infallible;

    use nonempty::NonEmpty;

    use zcash_client_backend::proposal::Proposal;
    use zcash_client_backend::wallet::NoteId;
    use zcash_client_backend::zip321::TransactionRequest;

    use zcash_primitives::transaction::fees::zip317;
    use zcash_primitives::transaction::TxId;

    use crate::data::proposal::ZingoProposal;
    use crate::lightclient::LightClient;
    use crate::wallet::propose::{ProposeSendError, ProposeShieldError};

    // TODO: untangle errors and fix send result so clone is not needed so we can impl from on std::error

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum BroadcastTransactionsError {
        #[error("Broadcast failed: {0:?}")]
        Broadcast(String),
        #[error("Transaction not found in the wallet: {0}")]
        TransactionNotFound(TxId),
        #[error("Transaction associated with given txid to broadcast does not have `Calculated` status: {0}")]
        IncorrectTransactionStatus(TxId),
        /// Failed to read transaction.
        #[error("Failed to read transaction.")]
        TransactionRead,
        /// Failed to write transaction.
        #[error("Failed to write transaction.")]
        TransactionWrite,
        /// Conversion failed
        #[error("Conversion failed. {0}")]
        ConversionFailed(#[from] crate::utils::error::ConversionError),
        /// No view capability
        #[error("No view capability")]
        NoViewCapability,
        /// Txid reported by server does not match calculated txid.
        #[error("Server error: txid reported by the server does not match calculated txid.\ncalculated txid:\n{0}\ntxid from server: {1}")]
        IncorrectTxidFromServer(TxId, TxId),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum CompleteAndBroadcastError {
        #[error("The transaction could not be calculated: {0:?}")]
        BuildTransaction(#[from] crate::wallet::send::BuildTransactionError),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(#[from] BroadcastTransactionsError),
        #[error("TxIds did not work through?")]
        EmptyList,
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum CompleteAndBroadcastStoredProposalError {
        #[error("No proposal. Call do_propose first.")]
        NoStoredProposal,
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum QuickSendError {
        #[error("propose send {0:?}")]
        ProposeSend(#[from] ProposeSendError),
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum QuickShieldError {
        #[error("propose shield {0:?}")]
        Propose(#[from] ProposeShieldError),
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    impl LightClient {
        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            let mut wallet = self.wallet.lock().await;
            let calculated_txids = wallet.create_transactions(proposal).await?;
            let broadcast_result = wallet
                .broadcast_calculated_transactions(self.get_server_uri(), calculated_txids)
                .await;

            wallet
                .set_send_result(broadcast_result.clone().map_err(|e| e.to_string()).map(
                    |vec_txids| {
                        serde_json::Value::Array(
                            vec_txids
                                .iter()
                                .map(|txid| serde_json::Value::String(txid.to_string()))
                                .collect::<Vec<serde_json::Value>>(),
                        )
                    },
                ))
                .await;

            let broadcast_txids = NonEmpty::from_vec(broadcast_result?)
                .ok_or(CompleteAndBroadcastError::EmptyList)?;

            Ok(broadcast_txids)
        }

        /// Calculates, signs and broadcasts transactions from a stored proposal.
        pub async fn complete_and_broadcast_stored_proposal(
            &self,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastStoredProposalError> {
            if let Some(proposal) = self.latest_proposal.read().await.as_ref() {
                match proposal {
                    ZingoProposal::Transfer(transfer_proposal) => {
                        self.complete_and_broadcast::<NoteId>(transfer_proposal)
                            .await
                    }
                    ZingoProposal::Shield(shield_proposal) => {
                        self.complete_and_broadcast::<Infallible>(shield_proposal)
                            .await
                    }
                }
                .map_err(CompleteAndBroadcastStoredProposalError::CompleteAndBroadcast)
            } else {
                Err(CompleteAndBroadcastStoredProposalError::NoStoredProposal)
            }
        }

        /// Creates, signs and broadcasts transactions from a transaction request without confirmation.
        pub async fn quick_send(
            &self,
            request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self
                .wallet
                .lock()
                .await
                .create_send_proposal(request)
                .await?;

            Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)
        }

        /// Shields all transparent funds without confirmation.
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.wallet.lock().await.create_shield_proposal().await?;

            Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)
        }
    }

    #[cfg(test)]
    mod test {
        // use zcash_client_backend::PoolType;

        // use crate::{
        //     lightclient::sync::test::sync_example_wallet,
        //     testutils::chain_generics::{
        //         conduct_chain::ConductChain as _, live_chain::LiveChain, with_assertions,
        //     },
        //     wallet::disk::testing::examples,
        // };

        // all tests below (and in this mod) use example wallets, which describe real-world chains.

        // FIXME: zingo2
        // #[tokio::test]
        // async fn complete_and_broadcast_unconnected_error() {
        //     use crate::{
        //         config::ZingoConfigBuilder, lightclient::LightClient,
        //         mocks::proposal::ProposalBuilder,
        //     };
        //     use testvectors::seeds::ABANDON_ART_SEED;
        //     let lc = LightClient::create_unconnected(
        //         &ZingoConfigBuilder::default().create(),
        //         crate::wallet::WalletBase::MnemonicPhrase(ABANDON_ART_SEED.to_string()),
        //         1,
        //     )
        //     .await
        //     .unwrap();
        //     let proposal = ProposalBuilder::default().build();
        //     lc.complete_and_broadcast(&proposal).await.unwrap_err();
        //     // TODO: match on specific error
        // }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send TESTNET: these assume the wallet has on-chain TAZ.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod testnet {
            // use super::*;

            // FIXME: zingo2
            // /// requires 1 confirmation: expect 3 minute runtime
            // #[ignore = "live testnet: testnet relies on NU6"]
            // #[tokio::test]
            // async fn glory_goddess_simple_send() {
            //     let case = examples::NetworkSeedVersion::Testnet(
            //         examples::TestnetSeedVersion::GloryGoddess,
            //     );
            //     let client = sync_example_wallet(case).await;

            //     with_assertions::assure_propose_shield_bump_sync(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         true,
            //     )
            //     .await
            //     .unwrap();
            // }

            // FIXME: zingo2
            // #[ignore = "live testnet: testnet relies on NU6"]
            // #[tokio::test]
            // /// this is a live sync test. its execution time scales linearly since last updated
            // /// this is a live send test. whether it can work depends on the state of live wallet on the blockchain
            // /// note: live send waits 2 minutes for confirmation. expect 3min runtime
            // async fn testnet_send_to_self_orchard() {
            //     let case = examples::NetworkSeedVersion::Testnet(
            //         examples::TestnetSeedVersion::ChimneyBetter(
            //             examples::ChimneyBetterVersion::Latest,
            //         ),
            //     );

            //     let client = sync_example_wallet(case).await;

            //     with_assertions::propose_send_bump_sync_all_recipients(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         vec![(
            //             &client,
            //             PoolType::Shielded(zcash_client_backend::ShieldedProtocol::Orchard),
            //             10_000,
            //             None,
            //         )],
            //         false,
            //     )
            //     .await
            //     .unwrap();
            // }

            // FIXME: zingo2
            // #[ignore = "live testnet: testnet relies on NU6"]
            // #[tokio::test]
            // /// this is a live sync test. its execution time scales linearly since last updated
            // /// note: live send waits 2 minutes for confirmation. expect 3min runtime
            // async fn testnet_shield() {
            //     let case = examples::NetworkSeedVersion::Testnet(
            //         examples::TestnetSeedVersion::ChimneyBetter(
            //             examples::ChimneyBetterVersion::Latest,
            //         ),
            //     );

            //     let client = sync_example_wallet(case).await;

            //     with_assertions::assure_propose_shield_bump_sync(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         true,
            //     )
            //     .await
            //     .unwrap();
            // }
        }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send MAINNET: spends on-chain ZEC.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod mainnet {
            // use super::*;

            // FIXME: zingo2
            // /// requires 1 confirmation: expect 3 minute runtime
            // #[tokio::test]
            // #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            // async fn mainnet_send_to_self_orchard() {
            //     let case = examples::NetworkSeedVersion::Mainnet(
            //         examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
            //     );
            //     let target_pool = PoolType::Shielded(ShieldedProtocol::Orchard);

            //     let client = sync_example_wallet(case).await;

            //     println!(
            //         "mainnet_hhcclaltpcckcsslpcnetblr has {} transactions in it",
            //         client
            //             .wallet
            //             .lock()
            //             .await
            //             .transaction_context
            //             .transaction_metadata_set
            //             .read()
            //             .await
            //             .transaction_records_by_id
            //             .len()
            //     );

            //     with_assertions::propose_send_bump_sync_all_recipients(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         vec![(&client, target_pool, 10_000, None)],
            //         false,
            //     )
            //     .await
            //     .unwrap();
            // }

            // FIXME: zingo2
            // /// requires 1 confirmation: expect 3 minute runtime
            // #[tokio::test]
            // #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            // async fn mainnet_send_to_self_sapling() {
            //     let case = examples::NetworkSeedVersion::Mainnet(
            //         examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
            //     );
            //     let target_pool = PoolType::Shielded(ShieldedProtocol::Sapling);

            //     let client = sync_example_wallet(case).await;

            //     println!(
            //         "mainnet_hhcclaltpcckcsslpcnetblr has {} transactions in it",
            //         client
            //             .wallet
            //             .lock()
            //             .await
            //             .transaction_context
            //             .transaction_metadata_set
            //             .read()
            //             .await
            //             .transaction_records_by_id
            //             .len()
            //     );

            //     with_assertions::propose_send_bump_sync_all_recipients(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         vec![(&client, target_pool, 400_000, None)],
            //         false,
            //     )
            //     .await
            //     .unwrap();
            // }

            // FIXME: zingo2
            // /// requires 2 confirmations: expect 6 minute runtime
            // #[tokio::test]
            // #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            // async fn mainnet_send_to_self_transparent_and_then_shield() {
            //     let case = examples::NetworkSeedVersion::Mainnet(
            //         examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
            //     );
            //     let target_pool = PoolType::Transparent;

            //     let client = sync_example_wallet(case).await;

            //     with_assertions::propose_send_bump_sync_all_recipients(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         vec![(&client, target_pool, 400_000, None)],
            //         false,
            //     )
            //     .await
            //     .unwrap();

            //     with_assertions::assure_propose_shield_bump_sync(
            //         &mut LiveChain::setup().await,
            //         &client,
            //         false,
            //     )
            //     .await
            //     .unwrap();
            // }
        }
    }
}
