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

    use zcash_primitives::consensus::{self, BlockHeight};
    use zcash_primitives::transaction::fees::zip317;
    use zcash_primitives::transaction::{Transaction, TxId};
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_sync::traits::SyncWallet;

    // use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::data::proposal::ZingoProposal;
    use crate::lightclient::LightClient;
    // use crate::wallet::now;
    use crate::wallet::propose::{ProposeSendError, ProposeShieldError};

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum TransactionCacheError {
        #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
        NoSpendCapability,
        #[error("No Tx in cached!")]
        NoCachedTx,
        #[error("Multistep transaction with non-tex steps")]
        InvalidMultiStep,
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum BroadcastCachedTransactionsError {
        #[error("Cant broadcast: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Transaction not recorded. Call record_created_transactions first: {0:?}")]
        Unrecorded(TxId),
        #[error("Couldnt fetch server height: {0:?}")]
        Height(String),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(String),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum RecordCachedTransactionsError {
        #[error("Cant record: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Couldnt fetch server height: {0:?}")]
        Height(String),
        #[error("Decoding failed: {0:?}")]
        Decode(#[from] std::io::Error),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, thiserror::Error)]
    pub enum CompleteAndBroadcastError {
        #[error("The transaction could not be calculated: {0:?}")]
        BuildTransaction(#[from] crate::wallet::send::BuildTransactionError),
        #[error("Recording created transaction failed: {0:?}")]
        Record(#[from] RecordCachedTransactionsError),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(#[from] BroadcastCachedTransactionsError),
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
        /// Broadcasts calculated transactions in order of time calculated and sets confirmation status to transmitted.
        async fn broadcast_created_transactions(
            &self,
        ) -> Result<Vec<TxId>, BroadcastCachedTransactionsError> {
            struct SentTransaction {
                txid: TxId,
                height: BlockHeight,
                transaction: Transaction,
            }

            let mut sent_transactions = Vec::new();
            let mut wallet = self.wallet.lock().await;
            let network = wallet.network;

            let mut calculated_transactions = wallet
                .wallet_transactions
                .values_mut()
                .filter(|transaction| {
                    matches!(transaction.status(), ConfirmationStatus::Calculated(_))
                })
                .collect::<Vec<_>>();

            calculated_transactions.sort_by_key(|transaction| transaction.datetime());

            for wallet_transaction in calculated_transactions {
                let height = wallet_transaction.status().get_height();

                let mut transaction_bytes = vec![];
                wallet_transaction
                    .transaction()
                    .write(&mut transaction_bytes)
                    .unwrap();
                let transaction = Transaction::read(
                    transaction_bytes.as_slice(),
                    consensus::BranchId::for_height(&network, height),
                )
                .unwrap();

                let txid_from_server = crate::grpc_connector::send_transaction(
                    self.get_server_uri(),
                    transaction_bytes.into_boxed_slice(),
                )
                .await
                .unwrap();

                let txid_from_server =
                    crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())
                        .unwrap();

                sent_transactions.push(SentTransaction {
                    txid: txid_from_server,
                    height,
                    transaction,
                });
            }

            let txids = sent_transactions
                .into_iter()
                .map(|sent_transaction| {
                    let txid_from_server = sent_transaction.txid;
                    let calculated_txid = sent_transaction.transaction.txid();

                if txid_from_server == calculated_txid {
                    zingo_sync::sync::scan_pending_transaction(
                        &network,
                        &wallet.get_unified_full_viewing_keys().unwrap(),
                        &mut *wallet,
                        sent_transaction.transaction,
                        ConfirmationStatus::Transmitted(sent_transaction.height),
                    );
                } else {
                    eprintln!(
                        "txid reported by the server does not match calulated txid.\ncalculated txid:\n{}\ntxid from server: {}",
                        calculated_txid, txid_from_server,
                    );
                    #[cfg(not(feature = "darkside_tests"))]
                    {
                        panic!("server error: server reported different txid to the one calculated!");
                    }
                    // during darkside tests, the server may report a different txid to the one calculated.
                    #[cfg(feature = "darkside_tests")]
                    {
                        todo!("find a way to override txids without unecessary kruft in sync engine")
                    }
                }


                    sent_transaction.txid
                })
                .collect();

            Ok(txids)
        }

        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            // FIXME: zingo2 manage wallet guard, move send methods to wallet with parameters for lightclient parts
            self.wallet
                .lock()
                .await
                .create_transaction(proposal)
                .await?;

            let broadcast_result = self.broadcast_created_transactions().await;

            self.wallet
                .lock()
                .await
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
            _request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            // FIXME: zingo2
            //     let proposal = self
            //         .wallet
            //         .lock()
            //         .await
            //         .create_send_proposal(request)
            //         .await?;
            //     Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)

            todo!()
        }

        /// Shields all transparent funds without confirmation.
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            // FIXME: zingo2
            //     let proposal = self.wallet.lock().await.create_shield_proposal().await?;
            //     Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)

            todo!()
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
