//! TODO: Add Mod Description Here!

use zcash_primitives::consensus::BlockHeight;

use super::LightClient;
use super::LightWalletSendProgress;

impl LightClient {
    async fn get_submission_height(&self) -> Result<BlockHeight, String> {
        Ok(BlockHeight::from_u32(
            crate::grpc_connector::get_latest_block(self.config.get_lightwalletd_uri())
                .await?
                .height as u32,
        ) + 1)
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

    use zcash_primitives::transaction::TxId;

    use thiserror::Error;

    use crate::lightclient::LightClient;
    use crate::wallet::propose::{ProposeSendError, ProposeShieldError};

    #[allow(missing_docs)] // error types document themselves
    #[derive(Clone, Debug, Error)]
    pub enum BroadcastCreatedTransactionsError {
        #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
        NoSpendCapability,
        #[error("No Tx to broadcast!")]
        NoTxCreated,
        #[error("Broadcast failed: {0:?}")]
        Broadcast(String),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastError {
        #[error("The transaction could not be calculated: {0:?}")]
        BuildTransaction(#[from] crate::wallet::send::BuildTransactionError),
        #[error("Cant get submission height. Server connection?: {0:?}")]
        SubmissionHeight(String),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(#[from] BroadcastCreatedTransactionsError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastStoredProposalError {
        #[error("No proposal. Call do_propose first.")]
        NoStoredProposal,
        #[error("send {0:?}")]
        CompleteAndBroadcast(CompleteAndBroadcastError),
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
        /// Calculates, signs and broadcasts transactions from a proposal.
        async fn broadcast_created_transactions(
            &self,
        ) -> Result<NonEmpty<TxId>, BroadcastCreatedTransactionsError> {
            let mut tx_map = self
                .wallet
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            match tx_map.spending_data_mut() {
                None => Err(BroadcastCreatedTransactionsError::NoSpendCapability),
                Some(ref mut spending_data) => {
                    let mut serverz_txids = vec![];
                    for (txid, raw_tx) in spending_data.cached_raw_transactions() {
                        match crate::grpc_connector::send_transaction(
                            self.get_server_uri(),
                            raw_tx.clone().into_boxed_slice(),
                        )
                        .await
                        {
                            Ok(_todo_compare_string) => serverz_txids.push(*txid),
                            Err(server_err) => {
                                return Err(BroadcastCreatedTransactionsError::Broadcast(
                                    server_err,
                                ))
                            }
                        };
                    }

                    let non_empty_serverz_txids = NonEmpty::from_vec(serverz_txids)
                        .ok_or(BroadcastCreatedTransactionsError::NoTxCreated)?;

                    Ok(non_empty_serverz_txids)
                }
            }
        }

        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            let submission_height = self
                .get_submission_height()
                .await
                .map_err(CompleteAndBroadcastError::SubmissionHeight)?;

            self.wallet.create_transaction(proposal).await?;

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

            Ok(broadcast_result?)
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
        use crate::{
            lightclient::LightClient,
            wallet::{
                disk::testing::examples::{
                    ExampleMSKMGDBHOTBPETCJWCSPGOPPWalletVersion, ExampleTestnetWalletSeed,
                    ExampleWalletNetwork,
                },
                LightWallet,
            },
        };

        #[tokio::test]
        async fn complete_and_broadcast_unconnected_error() {
            use crate::{
                config::ZingoConfigBuilder,
                lightclient::{send::send_with_proposal::CompleteAndBroadcastError, LightClient},
                mocks::ProposalBuilder,
                testvectors::seeds::ABANDON_ART_SEED,
            };
            let lc = LightClient::create_unconnected(
                &ZingoConfigBuilder::default().create(),
                crate::wallet::WalletBase::MnemonicPhrase(ABANDON_ART_SEED.to_string()),
                1,
            )
            .await
            .unwrap();
            let proposal = ProposalBuilder::default().build();
            assert_eq!(
                CompleteAndBroadcastError::SubmissionHeight(
                    "Error getting client: InvalidScheme".to_string(),
                )
                .to_string(),
                lc.complete_and_broadcast(&proposal)
                    .await
                    .unwrap_err()
                    .to_string(),
            );
        }

        #[ignore]
        #[tokio::test]
        async fn sync_testnet() {
            let wallet = LightWallet::load_example_wallet(ExampleWalletNetwork::Testnet(
                ExampleTestnetWalletSeed::MSKMGDBHOTBPETCJWCSPGOPP(
                    ExampleMSKMGDBHOTBPETCJWCSPGOPPWalletVersion::Gab72a38b,
                ),
            ))
            .await;
            let lc = LightClient::create_from_wallet_async(wallet).await.unwrap();
            let _ = lc.do_sync(true).await;
        }
    }
}
