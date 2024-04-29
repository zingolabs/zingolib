//! TODO: Add Mod Description Here!
use nonempty::NonEmpty;

use zcash_client_backend::address::Address;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;
use zcash_primitives::transaction::TxId;
use zcash_proofs::prover::LocalTxProver;

use crate::utils::zatoshis_from_u64;
use crate::wallet::Pool;

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

    /// Send funds
    pub async fn do_send(
        &self,
        receivers: Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>,
    ) -> Result<NonEmpty<TxId>, String> {
        self.do_propose_spend(receivers)
            .await
            .map_err(|e| e.to_string())?;
        self.do_send_proposed().await.map_err(|e| e.to_string())
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
        })
    }

    /// Shield funds. Send transparent or sapling funds to a unified address.
    /// Defaults to the unified address of the capability if `address` is `None`.
    pub async fn do_shield(
        &self,
        pools_to_shield: &[Pool],
        address: Option<Address>,
    ) -> Result<TxId, String> {
        let transaction_submission_height = self.get_submission_height().await?;
        let fee = u64::from(MINIMUM_FEE); // TODO: This can no longer be hard coded, and must be calced
                                          // as a fn of the transactions structure.
        let tbal = self
            .wallet
            .tbalance(None)
            .await
            .expect("to receive a balance");
        let sapling_bal = self
            .wallet
            .spendable_sapling_balance(None)
            .await
            .unwrap_or(0);

        // Make sure there is a balance, and it is greater than the amount
        let balance_to_shield = if pools_to_shield.contains(&Pool::Transparent) {
            tbal
        } else {
            0
        } + if pools_to_shield.contains(&Pool::Sapling) {
            sapling_bal
        } else {
            0
        };
        if balance_to_shield <= fee {
            return Err(format!(
                "Not enough transparent/sapling balance to shield. Have {} zats, need more than {} zats to cover tx fee",
                balance_to_shield, fee
            ));
        }

        let address = address.unwrap_or(Address::from(
            self.wallet.wallet_capability().addresses()[0].clone(),
        ));
        let amount = zatoshis_from_u64(balance_to_shield - fee)
            .expect("balance cannot be outside valid range of zatoshis");
        let receiver = vec![(address, amount, None)];

        let _lock = self.sync_lock.lock().await;
        let (sapling_output, sapling_spend) = self.read_sapling_params()?;

        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        self.wallet
            .send_to_addresses(
                sapling_prover,
                pools_to_shield.to_vec(),
                receiver,
                transaction_submission_height,
                |transaction_bytes| {
                    crate::grpc_connector::send_transaction(
                        self.get_server_uri(),
                        transaction_bytes,
                    )
                },
            )
            .await
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

        use std::ops::DerefMut;

        use zcash_keys::keys::UnifiedSpendingKey;

        if let Some(proposal) = self.latest_proposal.read().await.as_ref() {
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
                    let mut step_results = Vec::with_capacity(transfer_proposal.steps().len());
                    let mut txids = Vec::with_capacity(transfer_proposal.steps().len());
                    for step in transfer_proposal.steps() {
                        let mut tmamt = self
                            .wallet
                            .transaction_context
                            .transaction_metadata_set
                            .write()
                            .await;

                        let step_result =
                            zcash_client_backend::data_api::wallet::calculate_proposed_transaction(
                                tmamt.deref_mut(),
                                &self.wallet.transaction_context.config.chain,
                                &sapling_prover,
                                &sapling_prover,
                                &unified_spend_key,
                                zcash_client_backend::wallet::OvkPolicy::Sender,
                                transfer_proposal.fee_rule(),
                                transfer_proposal.min_target_height(),
                                &step_results,
                                step,
                            )
                            .map_err(DoSendProposedError::Calculation)?;
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
                crate::lightclient::ZingoProposal::Shield(_) => {
                    todo!();
                    // Ok(vec![TxId::from_bytes([222u8; 32])])
                }
            }
        } else {
            Err(DoSendProposedError::NoProposal)
        }
    }
}

use thiserror::Error;

/// Errors that can result from do_propose
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum DoSendProposedError {
    #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
    NoSpendCapability,
    #[error("No proposal. Call do_propose first.")]
    NoProposal,
    #[error("Cant get submission height. Server connection?: {}", {0})]
    SubmissionHeight(String),
    #[error("Could not load sapling_params: {}", {0})]
    SaplingParams(String),
    #[error("Could not find UnifiedSpendKey: {}", {0})]
    UnifiedSpendKey(std::io::Error),
    #[error("No proposal. Call do_propose first.")]
    Calculation(
        zcash_client_backend::data_api::error::Error<
            crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError,
            std::convert::Infallible,
            std::convert::Infallible,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Broadcast failed: {}", {0})]
    Broadcast(String),
}
