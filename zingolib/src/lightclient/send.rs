//! TODO: Add Mod Description Here!
use std::ops::DerefMut;

use nonempty::NonEmpty;

use sapling_crypto::prover::OutputProver;
use sapling_crypto::prover::SpendProver;

use zcash_keys::keys::UnifiedSpendingKey;

use zcash_client_backend::{proposal::Proposal, zip321::TransactionRequest};
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};
use zcash_proofs::prover::LocalTxProver;

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

    async fn iterate_proposal_send_scan<NoteRef>(
        &self,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        sapling_prover: &(impl SpendProver + OutputProver),
        unified_spend_key: &UnifiedSpendingKey,
        submission_height: BlockHeight,
    ) -> Result<NonEmpty<TxId>, DoSendProposedError> {
        let mut step_results = Vec::with_capacity(proposal.steps().len());
        let mut txids = Vec::with_capacity(proposal.steps().len());
        for step in proposal.steps() {
            let step_result = {
                let mut tmamt = self
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .write()
                    .await;

                zcash_client_backend::data_api::wallet::calculate_proposed_transaction(
                    tmamt.deref_mut(),
                    &self.wallet.transaction_context.config.chain,
                    sapling_prover,
                    sapling_prover,
                    unified_spend_key,
                    zcash_client_backend::wallet::OvkPolicy::Sender,
                    proposal.fee_rule(),
                    proposal.min_target_height(),
                    &step_results,
                    step,
                )
                .map_err(DoSendProposedError::Calculation)?
            };

            let txid = self
                .wallet
                .send_to_addresses_inner(
                    step_result.transaction(),
                    submission_height,
                    |transaction_bytes| {
                        crate::grpc_connector::send_transaction(
                            self.get_server_uri(),
                            transaction_bytes,
                        )
                    },
                )
                .await
                .map_err(DoSendProposedError::Broadcast)?;
            step_results.push((step, step_result));
            txids.push(txid);
        }
        Ok(NonEmpty::from_vec(txids).expect("nonempty"))
    }

    /// Unstable function to expose the zip317 interface for development
    // TODO: add correct functionality and doc comments / tests
    pub async fn do_send_proposed(&self) -> Result<NonEmpty<TxId>, DoSendProposedError> {
        if self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .is_none()
        {
            return Err(DoSendProposedError::NoSpendCapability);
        }

        if let Some(proposal) = self.latest_proposal.read().await.as_ref() {
            // fetch parameters for all outgoing transactions
            let submission_height = self
                .get_submission_height()
                .await
                .map_err(DoSendProposedError::SubmissionHeight)?;

            let (sapling_output, sapling_spend) = self
                .read_sapling_params()
                .map_err(DoSendProposedError::SaplingParams)?;
            let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
            let unified_spend_key =
                UnifiedSpendingKey::try_from(self.wallet.wallet_capability().as_ref())
                    .map_err(DoSendProposedError::UnifiedSpendKey)?;

            match proposal {
                crate::lightclient::ZingoProposal::Transfer(transfer_proposal) => {
                    self.iterate_proposal_send_scan(
                        transfer_proposal,
                        &sapling_prover,
                        &unified_spend_key,
                        submission_height,
                    )
                    .await
                }
                crate::lightclient::ZingoProposal::Shield(shield_proposal) => {
                    self.iterate_proposal_send_scan(
                        shield_proposal,
                        &sapling_prover,
                        &unified_spend_key,
                        submission_height,
                    )
                    .await
                }
            }
        } else {
            panic!("{:?}", DoSendProposedError::NoProposal)
        }
    }

    /// Send funds
    pub async fn do_quick_send(
        &self,
        request: TransactionRequest,
    ) -> Result<NonEmpty<TxId>, String> {
        self.do_propose_send(request)
            .await
            .map_err(|e| e.to_string())?;
        self.do_send_proposed().await.map_err(|e| e.to_string())
    }

    /// Send funds
    pub async fn do_quick_shield(&self) -> Result<NonEmpty<TxId>, String> {
        self.do_propose_shield().await.map_err(|e| e.to_string())?;
        self.do_send_proposed().await.map_err(|e| e.to_string())
    }
}

use thiserror::Error;

/// Errors that can result from do_send_proposed
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum DoSendProposedError {
    #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
    NoSpendCapability,
    #[error("No proposal. Call do_propose first.")]
    NoProposal,
    #[error("Cant get submission height. Server connection?: {0}")]
    SubmissionHeight(String),
    #[error("Could not load sapling_params: {0}")]
    SaplingParams(String),
    #[error("Could not find UnifiedSpendKey: {0}")]
    UnifiedSpendKey(std::io::Error),
    #[error("Can't Calculate {0}")]
    Calculation(
        zcash_client_backend::data_api::error::Error<
            crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError,
            std::convert::Infallible,
            std::convert::Infallible,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Broadcast failed: {0}")]
    Broadcast(String),
}

/// Errors that can result from do_quick_send
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum DoQuickSendProposedError {
    #[error("propose {0}")]
    Propose(crate::lightclient::propose::DoProposeError),
    #[error("No proposal. Call do_propose first.")]
    Send(DoSendProposedError),
}
