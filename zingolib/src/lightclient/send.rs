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

    use crate::lightclient::propose::{ProposeSendError, ProposeShieldError};
    use crate::lightclient::LightClient;

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastError {
        #[error("The transaction could not be calculated: {0:?}")]
        BuildTransaction(#[from] crate::wallet::send::BuildTransactionError),
        #[error("No proposal. Call do_propose first.")]
        NoProposal,
        #[error("Cant get submission height. Server connection?: {0:?}")]
        SubmissionHeight(String),
        #[error("Could not load sapling_params: {0:?}")]
        SaplingParams(String),
        #[error("Could not find UnifiedSpendKey: {0:?}")]
        UnifiedSpendKey(std::io::Error),
        #[error("Can't Calculate {0:?}")]
        Calculation(
            #[from]
            zcash_client_backend::data_api::error::Error<
                crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError,
                std::convert::Infallible,
                std::convert::Infallible,
                zcash_primitives::transaction::fees::zip317::FeeError,
            >,
        ),
        #[error("Broadcast failed: {0:?}")]
        Broadcast(String),
        #[error("Sending to exchange addresses is not supported yet!")]
        ExchangeAddressesNotSupported,
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
        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            let submission_height = self
                .get_submission_height()
                .await
                .map_err(CompleteAndBroadcastError::SubmissionHeight)?;

            let build_result = self.wallet.build_transaction(proposal).await?;

            let result = self
                .wallet
                .send_to_addresses_inner(
                    build_result.transaction(),
                    submission_height,
                    |transaction_bytes| {
                        crate::grpc_connector::send_transaction(
                            self.get_server_uri(),
                            transaction_bytes,
                        )
                    },
                )
                .await
                .map_err(CompleteAndBroadcastError::Broadcast)
                .map(NonEmpty::singleton);

            self.wallet
                .set_send_result(
                    result
                        .as_ref()
                        .map(|txids| txids.first().to_string())
                        .map_err(|e| e.to_string()),
                )
                .await;

            result
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
            let proposal = self.create_send_proposal(request).await?;
            Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)
        }

        /// Shields all transparent funds without confirmation.
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.create_shield_proposal().await?;
            Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)
        }
    }

    #[cfg(all(test, feature = "testvectors"))]
    mod tests {

        #[tokio::test]
        async fn complete_and_broadcast() {
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
    }
}
