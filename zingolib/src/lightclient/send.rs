//! TODO: Add Mod Description Here!
use log::debug;

use zcash_client_backend::address::Address;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;
use zcash_primitives::transaction::TxId;
use zcash_proofs::prover::LocalTxProver;

use crate::data::receivers::Receivers;
use crate::utils::conversion::zatoshis_from_u64;
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
    pub async fn do_send(&self, receivers: Receivers) -> Result<TxId, String> {
        let transaction_submission_height = self.get_submission_height().await?;
        // First, get the consensus branch ID
        debug!("Creating transaction");

        let _lock = self.sync_lock.lock().await;
        // I am not clear on how long this operation may take, but it's
        // clearly unnecessary in a send that doesn't include sapling
        // TODO: Remove from sends that don't include Sapling
        let (sapling_output, sapling_spend) = self.read_sapling_params()?;

        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        self.wallet
            .send_to_addresses(
                sapling_prover,
                vec![crate::wallet::Pool::Orchard, crate::wallet::Pool::Sapling], // This policy doesn't allow
                // spend from transparent.
                receivers,
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

        let recipient_address = address.unwrap_or(Address::from(
            self.wallet.wallet_capability().addresses()[0].clone(),
        ));
        let amount = zatoshis_from_u64(balance_to_shield - fee)
            .expect("balance cannot be outside valid range of zatoshis");
        let receivers = vec![crate::data::receivers::Receiver {
            recipient_address,
            amount,
            memo: None,
        }];

        let _lock = self.sync_lock.lock().await;
        let (sapling_output, sapling_spend) = self.read_sapling_params()?;

        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        self.wallet
            .send_to_addresses(
                sapling_prover,
                pools_to_shield.to_vec(),
                receivers,
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
}

#[cfg(feature = "zip317")]
/// patterns for newfangled propose flow
pub mod send_with_proposal {
    use std::convert::Infallible;

    use nonempty::NonEmpty;

    use zcash_client_backend::proposal::Proposal;
    use zcash_client_backend::wallet::NoteId;
    use zcash_primitives::transaction::TxId;

    use thiserror::Error;

    use crate::data::receivers::Receivers;
    use crate::lightclient::propose::{ProposeSendError, ProposeShieldError};
    use crate::lightclient::LightClient;

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastError {
        #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
        NoSpendCapability,
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum CompleteAndBroadcastStoredProposal {
        #[error("No proposal. Call do_propose first.")]
        NoStoredProposal,
        #[error("send {0}")]
        CompleteAndBroadcast(CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum QuickSendError {
        #[error("propose send {0}")]
        ProposeSend(ProposeSendError),
        #[error("send {0}")]
        CompleteAndBroadcast(CompleteAndBroadcastError),
    }

    #[allow(missing_docs)] // error types document themselves
    #[derive(Debug, Error)]
    pub enum QuickShieldError {
        #[error("propose shield {0}")]
        Propose(ProposeShieldError),
        #[error("send {0}")]
        CompleteAndBroadcast(CompleteAndBroadcastError),
    }

    impl LightClient {
        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        async fn complete_and_broadcast<NoteRef>(
            &self,
            _proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
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
            //todo!();
            Ok(NonEmpty::singleton(TxId::from_bytes([222u8; 32])))
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
            receivers: Receivers,
        ) -> Result<NonEmpty<TxId>, QuickSendError> {
            let proposal = self
                .propose_send(receivers)
                .await
                .map_err(QuickSendError::ProposeSend)?;
            self.complete_and_broadcast::<NoteId>(&proposal)
                .await
                .map_err(QuickSendError::CompleteAndBroadcast)
        }

        /// Unstable function to expose the zip317 interface for development
        // TODO: add correct functionality and doc comments / tests
        pub async fn quick_shield(&self) -> Result<NonEmpty<TxId>, QuickShieldError> {
            let proposal = self
                .propose_shield()
                .await
                .map_err(QuickShieldError::Propose)?;
            self.complete_and_broadcast::<Infallible>(&proposal)
                .await
                .map_err(QuickShieldError::CompleteAndBroadcast)
        }
    }
}
