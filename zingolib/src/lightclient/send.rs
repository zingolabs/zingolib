//! TODO: Add Mod Description Here!

use zcash_primitives::consensus::BlockHeight;

use super::LightClient;
use super::LightWalletSendProgress;

impl LightClient {
    async fn get_latest_block(&self) -> Result<BlockHeight, String> {
        Ok(BlockHeight::from_u32(
            crate::grpc_connector::get_latest_block(self.config.get_lightwalletd_uri())
                .await?
                .height as u32,
        ))
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
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

    use zcash_primitives::transaction::{Transaction, TxId};

    use thiserror::Error;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::lightclient::LightClient;
    use crate::wallet::now;
    use crate::wallet::propose::{ProposeSendError, ProposeShieldError};

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, Error)]
    pub enum TransactionCacheError {
        #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
        NoSpendCapability,
        #[error("No Tx in cached!")]
        NoCachedTx,
        #[error("Multistep transaction with non-tex steps")]
        InvalidMultiStep,
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, Error)]
    pub enum BroadcastCachedTransactionsError {
        #[error("Cant broadcast: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Couldnt fetch server height: {0:?}")]
        Height(String),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(String),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum RecordCachedTransactionsError {
        #[error("Cant record: {0:?}")]
        Cache(#[from] TransactionCacheError),
        #[error("Couldnt fetch server height: {0:?}")]
        Height(String),
        #[error("Decoding failed: {0:?}")]
        Decode(#[from] std::io::Error),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
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
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastStoredProposalError {
        #[error("No proposal. Call do_propose first.")]
        NoStoredProposal,
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum QuickSendError {
        #[error("propose send {0:?}")]
        ProposeSend(#[from] ProposeSendError),
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum QuickShieldError {
        #[error("propose shield {0:?}")]
        Propose(#[from] ProposeShieldError),
        #[error("send {0:?}")]
        CompleteAndBroadcast(#[from] CompleteAndBroadcastError),
    }

    impl LightClient {
        /// When a transactions are created, they are added to "spending_data".
        /// This step records all cached transactions into TransactionRecord s.
        /// This overwrites confirmation status to Calculated (not Broadcast)
        /// so only call this immediately after creating the transaction
        ///
        /// With the introduction of multistep transacations to support ZIP320
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
                .get_latest_block()
                .await
                .map_err(RecordCachedTransactionsError::Height)?;
            let mut transactions_to_record = vec![];
            if let Some(spending_data) = tx_map.spending_data_mut() {
                for (_txid, raw_tx) in spending_data.cached_raw_transactions().iter() {
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

        /// When a transaction is created, it is added to a cache. This step broadcasts the cache and sets its status to transmitted.
        /// only broadcasts transactions marked as calculated (not broadcast). when it broadcasts them, it marks them as broadcast.
        async fn broadcast_created_transactions(
            &self,
        ) -> Result<Vec<TxId>, BroadcastCachedTransactionsError> {
            let mut tx_map = self
                .wallet
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            let current_height = self
                .get_latest_block()
                .await
                .map_err(BroadcastCachedTransactionsError::Height)?;
            let calculated_tx_cache = tx_map
                .spending_data()
                .as_ref()
                .ok_or(BroadcastCachedTransactionsError::Cache(
                    TransactionCacheError::NoSpendCapability,
                ))?
                .cached_raw_transactions()
                .clone();
            let mut txids = vec![];
            for (txid, raw_tx) in calculated_tx_cache {
                let mut spend_status = None;
                // only send the txid if its status is Calculated. when we do, change its status to Transmitted.
                if let Some(transaction_record) = tx_map.transaction_records_by_id.get_mut(&txid) {
                    if matches!(transaction_record.status, ConfirmationStatus::Calculated(_)) {
                        match crate::grpc_connector::send_transaction(
                            self.get_server_uri(),
                            raw_tx.into_boxed_slice(),
                        )
                        .await
                        {
                            Ok(serverz_txid_string) => {
                                txids.push(crate::utils::txid::compare_txid_to_string(
                                    txid,
                                    serverz_txid_string,
                                    self.wallet.transaction_context.config.accept_server_txids,
                                ));
                                transaction_record.status =
                                    ConfirmationStatus::Transmitted(current_height + 1);

                                spend_status =
                                    Some((transaction_record.txid, transaction_record.status));
                            }
                            Err(server_err) => {
                                return Err(BroadcastCachedTransactionsError::Broadcast(server_err))
                            }
                        };
                    }
                }
                if let Some(s) = spend_status {
                    tx_map
                        .transaction_records_by_id
                        .update_note_spend_statuses(s.0, spend_status);
                }
            }

            tx_map
                .spending_data_mut()
                .as_mut()
                .ok_or(BroadcastCachedTransactionsError::Cache(
                    TransactionCacheError::NoSpendCapability,
                ))?
                .cached_raw_transactions_mut()
                .clear();

            Ok(txids)
        }

        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            self.wallet.create_transaction(proposal).await?;

            self.record_created_transactions().await?;

            let broadcast_result = self.broadcast_created_transactions().await;

            self.wallet
                .set_send_result(broadcast_result.clone().map_err(|e| e.to_string()).map(
                    |vec_txids| {
                        vec_txids
                            .iter()
                            .map(|txid| "created txid: ".to_string() + &txid.to_string())
                            .collect::<Vec<String>>()
                            .join(" & ")
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

        /// Creates, signs and broadcasts transactions from a transaction request without confirmation.
        pub async fn quick_send(
            &self,
            request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self.wallet.create_send_proposal(request).await?;
            Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)
        }

        /// Shields all transparent funds without confirmation.
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.wallet.create_shield_proposal().await?;
            Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)
        }
    }

    #[cfg(all(test, feature = "testvectors"))]
    mod tests {
        use zcash_client_backend::PoolType;

        use crate::{
            lightclient::sync::test::sync_example_wallet,
            testutils::chain_generics::{
                conduct_chain::ConductChain as _, live_chain::LiveChain, with_assertions,
            },
            wallet::disk::testing::examples::{
                ExampleCBBHRWIILGBRABABSSHSMTPRVersion, ExampleMSKMGDBHOTBPETCJWCSPGOPPVersion,
                ExampleTestnetWalletSeed, ExampleWalletNetwork,
            },
        };

        #[tokio::test]
        async fn complete_and_broadcast_unconnected_error() {
            use crate::{
                config::ZingoConfigBuilder, lightclient::LightClient,
                mocks::proposal::ProposalBuilder, testvectors::seeds::ABANDON_ART_SEED,
            };
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

        #[ignore = "live testnet"]
        #[tokio::test]
        /// this is a live sync test. its execution time scales linearly since last updated
        /// this is a live send test. whether it can work depends on the state of live wallet on the blockchain
        /// this wallet contains archaic diversified addresses, which may clog the new send engine.
        async fn testnet_mskmgdbhotbpetcjwcspgopp_shield_multi_account() {
            std::env::set_var("RUST_BACKTRACE", "1");
            let client = crate::lightclient::sync::test::sync_example_wallet(
                ExampleWalletNetwork::Testnet(ExampleTestnetWalletSeed::MSKMGDBHOTBPETCJWCSPGOPP(
                    ExampleMSKMGDBHOTBPETCJWCSPGOPPVersion::Ga74fed621,
                )),
            )
            .await;

            with_assertions::propose_shield_bump_sync(&mut LiveChain::setup().await, &client, true)
                .await;
        }

        #[ignore = "live testnet"]
        #[tokio::test]
        /// this is a live sync test. its execution time scales linearly since last updated
        /// this is a live send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_cbbhrwiilgbrababsshsmtpr_send_to_self_orchard_hot() {
            std::env::set_var("RUST_BACKTRACE", "1");
            let client = sync_example_wallet(ExampleWalletNetwork::Testnet(
                ExampleTestnetWalletSeed::CBBHRWIILGBRABABSSHSMTPR(
                    ExampleCBBHRWIILGBRABABSSHSMTPRVersion::G2f3830058,
                ),
            ))
            .await;

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
            .await;
        }

        #[ignore = "live testnet"]
        #[tokio::test]
        /// this is a live sync test. its execution time scales linearly since last updated
        async fn testnet_cbbhrwiilgbrababsshsmtpr_shield_hot() {
            std::env::set_var("RUST_BACKTRACE", "1");
            let client = sync_example_wallet(ExampleWalletNetwork::Testnet(
                ExampleTestnetWalletSeed::CBBHRWIILGBRABABSSHSMTPR(
                    ExampleCBBHRWIILGBRABABSSHSMTPRVersion::G2f3830058,
                ),
            ))
            .await;

            with_assertions::propose_shield_bump_sync(&mut LiveChain::setup().await, &client, true)
                .await;
        }
    }
}
