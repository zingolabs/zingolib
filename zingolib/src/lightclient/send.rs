//! TODO: Add Mod Description Here!

use zcash_primitives::consensus::BlockHeight;

use super::LightClient;
use super::LightWalletSendProgress;

impl LightClient {
    pub(crate) async fn get_latest_block_height(&self) -> Result<BlockHeight, String> {
        crate::grpc_connector::get_latest_block_height(&self.config.get_lightwalletd_uri()).await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let result = self.wallet.get_send_result().await;
        todo!();
        Ok(LightWalletSendProgress {
            progress: result.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
        })
    }
}

/// patterns for newfangled propose flow
pub mod send_with_proposal {
    use std::convert::Infallible;
    use std::sync::Arc;

    use nonempty::NonEmpty;

    use tokio::sync::RwLock;
    use zcash_client_backend::proposal::Proposal;
    use zcash_client_backend::wallet::NoteId;
    use zcash_client_backend::zip321::TransactionRequest;

    use zcash_primitives::consensus::BlockHeight;
    use zcash_primitives::transaction::{Transaction, TxId};

    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::data::{txid_comparison, TxIdComparisonError};
    use crate::lightclient::LightClient;
    use crate::wallet::propose::{ProposeSendError, ProposeShieldError};
    use crate::wallet::transaction_records_by_id::GetRecordError;
    use crate::wallet::tx_map::TxMap;
    use crate::wallet::{now, SendProgress};

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
    pub enum RecordCachedTransactionsError {
        #[error("Cant record: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Couldnt fetch server height: {0:?}")]
        Height(String),
        #[error("Decoding failed: {0:?}")]
        Decode(#[from] std::io::Error),
    }

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

    /// please note these functions are now arranged in order of top-level to lower level.
    impl LightClient {
        /// Creates, signs and broadcasts transactions from a transaction request without confirmation.
        /// A primary way to ask the LightClient to send.
        pub async fn quick_send(
            &self,
            request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self.wallet.create_send_proposal(request).await?;
            Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)
        }

        /// Shields all transparent funds without confirmation.
        /// A second way to ask the LightClient to send. Will only send transparent funds, to self.
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.wallet.create_shield_proposal().await?;
            Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)
        }

        /// Calculates, signs and broadcasts transactions from a stored proposal.
        /// A third way to ask the LightClient to send. Will finalize any already-proposed transaction.
        pub async fn complete_and_broadcast_stored_proposal(
            &self,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastStoredProposalError> {
            if let Some(proposal) = self.latest_proposal.read().await.as_ref() {
                match proposal {
                    crate::lightclient::ZingoProposal::Transfer(transfer_proposal) => {
                        self.complete_and_broadcast::<NoteId>(transfer_proposal)
                            .await
                    }
                    crate::lightclient::ZingoProposal::Shield(shield_proposal) => {
                        self.complete_and_broadcast::<Infallible>(shield_proposal)
                            .await
                    }
                }
                .map_err(CompleteAndBroadcastStoredProposalError::CompleteAndBroadcast)
            } else {
                Err(CompleteAndBroadcastStoredProposalError::NoStoredProposal)
            }
        }

        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            self.wallet.create_transaction(proposal).await?;

            let record_txids_result = self.record_created_transactions().await;
            let txids = NonEmpty::from_vec(record_txids_result?)
                .ok_or(CompleteAndBroadcastError::EmptyList)?;

            start_broadcast_loop(
                self.wallet
                    .transaction_context
                    .transaction_metadata_set
                    .clone(),
                self.get_server_uri(),
                self.wallet.send_progress.clone(),
            )
            .await;

            Ok(txids)
        }

        /// When a transactions are created, they are added to "spending_data".
        /// This step records all cached transactions into TransactionRecord s.
        /// This overwrites confirmation status to Calculated (not Broadcast)
        /// so only call this immediately after creating the transaction
        ///
        /// With the introduction of multistep transactions to support ZIP320
        /// we begin ordering transactions in the "spending_data" cache such
        /// that any output that's used to fund a subsequent transaction is
        /// added prior to that fund-requiring transaction.
        /// After some consideration we don't see why the spending_data should
        /// be stored out-of-order with respect to earlier transactions funding
        /// later ones in the cache, so we implement an in order cache.
        async fn record_created_transactions(
            &self,
        ) -> Result<Vec<TxId>, RecordCachedTransactionsError> {
            let mut tx_map = self
                .wallet
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            let current_height = self
                .get_latest_block_height()
                .await
                .map_err(RecordCachedTransactionsError::Height)?;
            let mut transactions_to_record = vec![];
            if let Some(spending_data) = &mut tx_map.spending_data {
                for (_txid, raw_tx) in spending_data.cached_raw_transactions.iter() {
                    transactions_to_record.push(Transaction::read(
                        raw_tx.as_slice(),
                        zcash_primitives::consensus::BranchId::for_height(
                            &self.wallet.transaction_context.config.chain,
                            current_height + 1,
                        ),
                    )?);
                }
            } else {
                return Err(RecordCachedTransactionsError::Cache(
                    TransactionCacheError::NoSpendCapability,
                ));
            }
            drop(tx_map);
            let mut txids = vec![];
            for transaction in transactions_to_record {
                self.wallet
                    .transaction_context
                    .scan_full_tx(
                        &transaction,
                        ConfirmationStatus::Calculated(current_height + 1),
                        Some(now() as u32),
                        crate::wallet::utils::get_price(
                            now(),
                            &self.wallet.price.read().await.clone(),
                        ),
                    )
                    .await;
                self.wallet
                    .transaction_context
                    .transaction_metadata_set
                    .write()
                    .await
                    .transaction_records_by_id
                    .update_note_spend_statuses(
                        transaction.txid(),
                        Some((
                            transaction.txid(),
                            ConfirmationStatus::Calculated(current_height + 1),
                        )),
                    );
                txids.push(transaction.txid());
            }
            Ok(txids)
        }
    }

    pub async fn start_broadcast_loop(
        arc_tx_map: Arc<RwLock<TxMap>>,
        server_uri: http::Uri,
        send_result: Arc<RwLock<SendProgress>>,
    ) {
        tokio::spawn(async move {
            loop {
                println!("broadcast attempt beginning");

                send_result.write().await.attempt += 1;

                let broadcast_result = dbg!(
                    broadcast_cached_transactions(arc_tx_map.clone(), server_uri.clone()).await
                );

                match broadcast_result {
                    Err(e) => {
                        send_result.write().await.last_result = Some(Err(e.to_string()));
                    }
                    Ok((txids, any_broadcast)) => {
                        if any_broadcast {
                            send_result.write().await.last_result = Some(Ok(txids));
                        } else {
                            // no transactions were broadcast because they were all confirmed
                            break;
                        }
                    }
                };

                if (dbg!(send_result.write().await.attempt) > 5) {
                    break;
                } else {
                    tokio::task::yield_now().await;
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        });
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum BroadcastCachedTransactionsError {
        #[error("Cant broadcast: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Transaction record should have been created already. {0:?}")]
        Record(#[from] GetRecordError),
        #[error("Broadcast incomplete. Error: {0:?}")]
        Incomplete(#[from] BroadcastTransactionsError),
    }

    /// When a transaction is created, it is added to a cache. This step broadcasts the cache and sets its status to transmitted.
    /// only broadcasts transactions marked as calculated (not broadcast). when it broadcasts them, it marks them as broadcast.
    /// the bool in the return denotes whether any transactions have been broadcast. if it is false, we conclude that the broadcast is finished.
    async fn broadcast_cached_transactions(
        arc_tx_map: Arc<RwLock<TxMap>>,
        server_uri: http::Uri,
    ) -> Result<(NonEmpty<TxId>, bool), BroadcastCachedTransactionsError> {
        let tx_map = arc_tx_map.write().await;
        let calculated_tx_cache = tx_map
            .spending_data
            .as_ref()
            .ok_or(BroadcastCachedTransactionsError::Cache(
                TransactionCacheError::NoSpendCapability,
            ))?
            .cached_raw_transactions
            .clone();
        drop(tx_map);
        Ok(broadcast_transactions(arc_tx_map, server_uri, calculated_tx_cache).await?)
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum BroadcastTransactionsError {
        #[error("Cant broadcast: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Transaction record should have been created already. {0:?}")]
        Record(#[from] GetRecordError),
        #[error("Broadcast incomplete. Error: {0:?}")]
        Incomplete(#[from] BroadcastTransactionError),
    }

    /// only broadcasts transactions marked as calculated (not broadcast). when it broadcasts them, it marks them as broadcast.
    /// the bool in the return denotes whether any transactions have been broadcast. if it is false, we conclude that the broadcast is finished.
    async fn broadcast_transactions(
        arc_tx_map: Arc<RwLock<TxMap>>,
        server_uri: http::Uri,
        transactions_to_broadcast: Vec<(TxId, Vec<u8>)>,
    ) -> Result<(NonEmpty<TxId>, bool), BroadcastTransactionsError> {
        let mut results = vec![];
        let mut any_transaction_broadcast = false;
        for (txid, raw_tx) in transactions_to_broadcast {
            let mut tx_map = arc_tx_map.write().await;

            let transaction_record = tx_map.transaction_records_by_id.get_record(&txid)?;
            // only send the txid if its status is Calculated. when we do, change its status to Transmitted.
            match transaction_record.status {
                ConfirmationStatus::Calculated(_) | ConfirmationStatus::Transmitted(_) => {
                    drop(tx_map);
                    broadcast_transaction(arc_tx_map.clone(), txid, raw_tx, &server_uri).await?;
                    any_transaction_broadcast = true;
                    results.push(txid);
                }
                ConfirmationStatus::Mempool(_) | ConfirmationStatus::Confirmed(_) => {}
            }
        }
        NonEmpty::from_vec(results)
            .map(|vec| (vec, any_transaction_broadcast))
            .ok_or(BroadcastTransactionsError::Cache(
                TransactionCacheError::NoCachedTx,
            ))
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum BroadcastTransactionError {
        #[error("Server returned error in response to blockheight query: {0:?}")]
        Height(String),
        #[error("LightServer returned error in response to broadcast: {0:?}")]
        ServerResponse(String),
        #[error("Broadcast transaction successfully, but errored in post-processing: {0:?}")]
        PostProcessingError(#[from] PostBroadcastSuccessUpdateTransactionError),
    }

    /// Ok(false) suggests the transaction was not broadcast because it was already on server.
    /// Ok(true) if it was broadcast
    async fn broadcast_transaction(
        arc_tx_map: Arc<RwLock<TxMap>>,
        txid: TxId,
        raw_tx: Vec<u8>,
        server_uri: &http::Uri,
    ) -> Result<(), BroadcastTransactionError> {
        let current_height = crate::grpc_connector::get_latest_block_height(server_uri)
            .await
            .map_err(BroadcastTransactionError::Height)?;
        println!("actually sending to server now");
        match crate::grpc_connector::send_transaction(server_uri.clone(), raw_tx.into_boxed_slice())
            .await
        {
            Err(server_err) => Err(BroadcastTransactionError::ServerResponse(server_err)),
            Ok(serverz_txid_string) => post_broadcast_success_update_transaction(
                arc_tx_map,
                &txid,
                serverz_txid_string,
                current_height,
            )
            .await
            .map_err(|e| e.into()),
        }
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, thiserror::Error)]
    pub enum PostBroadcastSuccessUpdateTransactionError {
        #[error("Transaction record should have been created already. {0:?}")]
        Record(#[from] GetRecordError),
        #[error("LightServer returned success txid string, but: {0:?}")]
        ServerResponse(#[from] TxIdComparisonError),
    }

    async fn post_broadcast_success_update_transaction(
        arc_tx_map: Arc<RwLock<TxMap>>,
        txid: &TxId,
        broadcast_success: String,
        current_height: BlockHeight,
    ) -> Result<(), PostBroadcastSuccessUpdateTransactionError> {
        let mut tx_map = arc_tx_map.write().await;

        let transaction_record = tx_map.transaction_records_by_id.get_record(txid)?;

        let new_status = ConfirmationStatus::Transmitted(current_height + 1);

        transaction_record.status = new_status;

        let chosen_txid: TxId = match txid_comparison(broadcast_success, txid) {
            #[cfg(feature = "darkside_tests")]
            Err(TxIdComparisonError::InconsistentTxId(reported_txid, known_txid)) => {
                // during darkside tests, the server may generate a new txid.
                // we accept and swap the TransactionRecord to the new txid
                if tx_map.reidentify_tx(known_txid, reported_txid).is_ok() {
                    reported_txid
                } else {
                    panic!()
                }
            }
            Ok(txid) => txid,
            Err(e) => {
                Err(e)?;
                panic!()
            }
        };

        let spend_status = Some((chosen_txid, new_status));
        tx_map
            .transaction_records_by_id
            .update_note_spend_statuses(chosen_txid, spend_status);

        Ok(())
    }

    // async fn broadcast_confirmed(
    //     arc_tx_map: Arc<RwLock<TxMap>>,
    //     txid: &TxId,
    //     current_height: BlockHeight,
    // ) -> Result<(), PostBroadcastSuccessUpdateTransactionError> {
    // }

    #[cfg(test)]
    mod test {
        use zcash_client_backend::{PoolType, ShieldedProtocol};

        use crate::{
            lightclient::sync::test::sync_example_wallet,
            testutils::chain_generics::{
                conduct_chain::ConductChain as _, live_chain::LiveChain, with_assertions,
            },
            wallet::disk::testing::examples,
        };

        // all tests below (and in this mod) use example wallets, which describe real-world chains.

        #[tokio::test]
        async fn complete_and_broadcast_unconnected_error() {
            use crate::{
                config::ZingoConfigBuilder, lightclient::LightClient,
                mocks::proposal::ProposalBuilder,
            };
            use testvectors::seeds::ABANDON_ART_SEED;
            let lc = LightClient::create_unconnected(
                &ZingoConfigBuilder::default().create(),
                crate::wallet::WalletBase::MnemonicPhrase(ABANDON_ART_SEED.to_string()),
                1,
            )
            .await
            .unwrap();
            let proposal = ProposalBuilder::default().build();
            lc.complete_and_broadcast(&proposal).await.unwrap_err();
            // TODO: match on specific error
        }

        /// live sync: execution time increases linearly until example wallet is upgraded
        /// live send TESTNET: these assume the wallet has on-chain TAZ.
        /// - waits 150 seconds for confirmation per transaction. see [zingolib/src/testutils/chain_generics/live_chain.rs]
        mod testnet {
            use super::*;

            /// requires 1 confirmation: expect 3 minute runtime
            #[ignore = "live testnet: testnet relies on NU6"]
            #[tokio::test]
            async fn glory_goddess_simple_send() {
                let case = examples::NetworkSeedVersion::Testnet(
                    examples::TestnetSeedVersion::GloryGoddess,
                );
                let client = sync_example_wallet(case).await;

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &client,
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

                let client = sync_example_wallet(case).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &client,
                    vec![(
                        &client,
                        PoolType::Shielded(zcash_client_backend::ShieldedProtocol::Orchard),
                        10_000,
                        None,
                    )],
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

                let client = sync_example_wallet(case).await;

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &client,
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
            use super::*;

            /// requires 1 confirmation: expect 3 minute runtime
            #[tokio::test]
            #[ignore = "dont automatically run hot tests! this test spends actual zec!"]
            async fn mainnet_send_to_self_orchard() {
                let case = examples::NetworkSeedVersion::Mainnet(
                    examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Latest),
                );
                let target_pool = PoolType::Shielded(ShieldedProtocol::Orchard);

                let client = sync_example_wallet(case).await;

                println!(
                    "mainnet_hhcclaltpcckcsslpcnetblr has {} transactions in it",
                    client
                        .wallet
                        .transaction_context
                        .transaction_metadata_set
                        .read()
                        .await
                        .transaction_records_by_id
                        .len()
                );

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &client,
                    vec![(&client, target_pool, 10_000, None)],
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
                let target_pool = PoolType::Shielded(ShieldedProtocol::Sapling);

                let client = sync_example_wallet(case).await;

                println!(
                    "mainnet_hhcclaltpcckcsslpcnetblr has {} transactions in it",
                    client
                        .wallet
                        .transaction_context
                        .transaction_metadata_set
                        .read()
                        .await
                        .transaction_records_by_id
                        .len()
                );

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &client,
                    vec![(&client, target_pool, 400_000, None)],
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
                let target_pool = PoolType::Transparent;

                let client = sync_example_wallet(case).await;

                with_assertions::propose_send_bump_sync_all_recipients(
                    &mut LiveChain::setup().await,
                    &client,
                    vec![(&client, target_pool, 400_000, None)],
                    false,
                )
                .await
                .unwrap();

                with_assertions::assure_propose_shield_bump_sync(
                    &mut LiveChain::setup().await,
                    &client,
                    false,
                )
                .await
                .unwrap();
            }
        }
    }
}
