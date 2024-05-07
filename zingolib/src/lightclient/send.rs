//! TODO: Add Mod Description Here!

use nonempty::NonEmpty;

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

    async fn update_tmamt_and_return_step_result<N>(
        &self,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, N>,
        step: zcash_client_backend::proposal::Step<N>,
        step_results: &Vec<(
            &zcash_client_backend::proposal::Step<N>,
            zcash_primitives::transaction::builder::BuildResult,
        )>,
    ) -> Result<zcash_primitives::transaction::builder::BuildResult, DoSendProposedError> {
        let fee_rule = proposal.fee_rule();
        let min_target_height = proposal.min_target_height();
        let unified_spend_key = zcash_keys::keys::UnifiedSpendingKey::try_from(
            self.wallet.wallet_capability().as_ref(),
        )
        .map_err(DoSendProposedError::UnifiedSpendKey)?;
        let (sapling_output, sapling_spend) = self
            .read_sapling_params()
            .map_err(DoSendProposedError::SaplingParams)?;
        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        zcash_client_backend::data_api::wallet::calculate_proposed_transaction(
            std::ops::DerefMut::deref_mut(
                &mut self
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .write()
                    .await,
            ),
            &self.wallet.transaction_context.config.chain,
            &sapling_prover,
            &sapling_prover,
            &unified_spend_key,
            zcash_client_backend::wallet::OvkPolicy::Sender,
            fee_rule,
            min_target_height,
            &step_results,
            &step,
        )
        .map_err(DoSendProposedError::Calculation)
    }
    async fn iterate_proposal_send_scan<NoteRef: Sized + Clone>(
        &self,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        submission_height: BlockHeight,
    ) -> Result<NonEmpty<TxId>, DoSendProposedError> {
        let mut step_results = Vec::with_capacity(proposal.steps().len());
        let mut txids = Vec::with_capacity(proposal.steps().len());
        for step in proposal.steps() {
            let step_result = self
                .update_tmamt_and_return_step_result(proposal, step.clone(), &step_results)
                .await?;

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

            match proposal {
                crate::lightclient::ZingoProposal::Transfer(transfer_proposal) => {
                    self.iterate_proposal_send_scan(transfer_proposal, submission_height)
                        .await
                }
                crate::lightclient::ZingoProposal::Shield(shield_proposal) => {
                    self.iterate_proposal_send_scan(shield_proposal, submission_height)
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
#[cfg(test)]
mod test {
    use zingo_testvectors::seeds::HOSPITAL_MUSEUM_SEED;
    use zingoconfig::ZingoConfigBuilder;

    use crate::{lightclient::LightClient, test_framework::mocks::ProposalBuilder};

    #[tokio::test]
    async fn update_tmamt_and_return_step_result() {
        let config = ZingoConfigBuilder::default().create();
        let client = LightClient::create_unconnected(
            &config,
            crate::wallet::WalletBase::MnemonicPhrase(HOSPITAL_MUSEUM_SEED.to_string()),
            0,
        )
        .await
        .expect("A client!");
        let proposal = ProposalBuilder::new().build();
        /*
        let step = zcash_client_backend::proposal::Step::from_parts();
        let step_result = client
            .update_tmamt_and_return_step_result(&proposal, step, step_results)
            .await;
        */
    }
}

/// Errors that can result from do_send_proposed
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
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
#[derive(Debug, thiserror::Error)]
pub enum DoQuickSendProposedError {
    #[error("propose {0}")]
    Propose(crate::lightclient::propose::DoProposeError),
    #[error("Can't QuickSend! No proposal. Call do_propose first.")]
    Send(DoSendProposedError),
}
