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
    use std::{convert::Infallible, ops::DerefMut as _};

    use hdwallet::traits::Deserialize as _;
    use nonempty::NonEmpty;

    use secp256k1::SecretKey;
    use zcash_client_backend::wallet::NoteId;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::{proposal::Proposal, wallet::TransparentAddressMetadata};
    use zcash_keys::keys::UnifiedSpendingKey;
    use zcash_primitives::transaction::TxId;

    use thiserror::Error;
    use zcash_proofs::prover::LocalTxProver;

    use crate::lightclient::LightClient;
    use crate::{
        lightclient::propose::{ProposeSendError, ProposeShieldError},
        wallet::utils::read_sapling_params,
    };

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastError {
        #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
        NoSpendCapability,
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
    pub enum CompleteAndBroadcastStoredProposal {
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
        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        async fn complete_and_broadcast<NoteRef>(
            &self,
            proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastError> {
            if self
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .is_none()
            {
                return Err(CompleteAndBroadcastError::NoSpendCapability);
            }

            // Reset the progress to start. Any errors will get recorded here
            self.wallet.reset_send_progress().await;

            let submission_height = self
                .get_submission_height()
                .await
                .map_err(CompleteAndBroadcastError::SubmissionHeight)?;

            let (sapling_output, sapling_spend): (Vec<u8>, Vec<u8>) =
                read_sapling_params().map_err(CompleteAndBroadcastError::SaplingParams)?;
            let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
            let unified_spend_key =
                UnifiedSpendingKey::try_from(self.wallet.wallet_capability().as_ref())
                    .map_err(CompleteAndBroadcastError::UnifiedSpendKey)?;

            // We don't support zip320 yet. Only one step.
            if proposal.steps().len() != 1 {
                return Err(CompleteAndBroadcastError::ExchangeAddressesNotSupported);
            }

            let step = proposal.steps().first();

            // The 'UnifiedSpendingKey' we create is not a 'proper' USK, in that the
            // transparent key it contains is not the account spending key, but the
            // externally-scoped derivative key. The goal is to fix this, but in the
            // interim we use this special-case logic.
            fn usk_to_tkey(
                unified_spend_key: &UnifiedSpendingKey,
                t_metadata: &TransparentAddressMetadata,
            ) -> SecretKey {
                hdwallet::ExtendedPrivKey::deserialize(&unified_spend_key.transparent().to_bytes())
                    .expect("This a hack to do a type conversion, and will not fail")
                    .derive_private_key(t_metadata.address_index().into())
                    // This is unwrapped in librustzcash, so I'm not too worried about it
                    .expect("private key derivation failed")
                    .private_key
            }

            let build_result =
                zcash_client_backend::data_api::wallet::calculate_proposed_transaction(
                    self.wallet
                        .transaction_context
                        .transaction_metadata_set
                        .write()
                        .await
                        .deref_mut(),
                    &self.wallet.transaction_context.config.chain,
                    &sapling_prover,
                    &sapling_prover,
                    &unified_spend_key,
                    zcash_client_backend::wallet::OvkPolicy::Sender,
                    proposal.fee_rule(),
                    proposal.min_target_height(),
                    &[],
                    step,
                    Some(usk_to_tkey),
                    Some(self.wallet.wallet_capability().first_sapling_address()),
                )
                .map_err(CompleteAndBroadcastError::Calculation)?;

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

        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        pub async fn complete_and_broadcast_stored_proposal(
            &self,
        ) -> Result<NonEmpty<TxId>, CompleteAndBroadcastStoredProposal> {
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
                .map_err(CompleteAndBroadcastStoredProposal::CompleteAndBroadcast)
            } else {
                Err(CompleteAndBroadcastStoredProposal::NoStoredProposal)
            }
        }

        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        pub async fn quick_send(
            &self,
            request: TransactionRequest,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self.create_send_proposal(request).await?;
            Ok(self.complete_and_broadcast::<NoteId>(&proposal).await?)
        }

        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self.create_shield_proposal().await?;
            Ok(self.complete_and_broadcast::<Infallible>(&proposal).await?)
        }
    }

    #[cfg(test)]
    mod tests {
        use zingo_testvectors::seeds::ABANDON_ART_SEED;
        use zingoconfig::ZingoConfigBuilder;

        use crate::{
            lightclient::{send::send_with_proposal::CompleteAndBroadcastError, LightClient},
            mocks::ProposalBuilder,
        };

        #[tokio::test]
        async fn complete_and_broadcast() {
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
