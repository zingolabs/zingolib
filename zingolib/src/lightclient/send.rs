//! TODO: Add Mod Description Here!
use log::{debug, error};

use zcash_primitives::{
    consensus::BlockHeight,
    memo::MemoBytes,
    transaction::{components::amount::NonNegativeAmount, fees::zip317::MINIMUM_FEE},
};
use zcash_proofs::prover::LocalTxProver;

use super::{LightClient, LightWalletSendProgress};
use crate::wallet::Pool;

#[cfg(feature = "zip317")]
use {
    crate::wallet::notes::NoteRecordIdentifier,
    zcash_client_backend::proposal::Proposal,
    zcash_primitives::transaction::{fees::zip317::FeeRule, TxId},
};

impl LightClient {
    async fn get_submission_height(&self) -> Result<BlockHeight, String> {
        Ok(BlockHeight::from_u32(
            crate::grpc_connector::get_latest_block(self.config.get_lightwalletd_uri())
                .await?
                .height as u32,
        ) + 1)
    }

    fn map_tos_to_receivers(
        &self,
        tos: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<
        Vec<(
            zcash_client_backend::address::Address,
            NonNegativeAmount,
            Option<MemoBytes>,
        )>,
        String,
    > {
        if tos.is_empty() {
            return Err("Need at least one destination address".to_string());
        }
        tos.iter()
            .map(|to| {
                let ra = match zcash_client_backend::address::Address::decode(
                    &self.config.chain,
                    to.0,
                ) {
                    Some(to) => to,
                    None => {
                        let e = format!("Invalid recipient address: '{}'", to.0);
                        error!("{}", e);
                        return Err(e);
                    }
                };

                let value = NonNegativeAmount::from_u64(to.1).unwrap();

                Ok((ra, value, to.2.clone()))
            })
            .collect()
    }

    /// Unstable function to expose the zip317 interface for development
    // TODO: add correct functionality and doc comments / tests
    // TODO: Add migrate_sapling_to_orchard argument
    #[cfg(feature = "zip317")]
    pub async fn do_propose(
        &self,
        _address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<Proposal<FeeRule, NoteRecordIdentifier>, String> {
        use crate::test_framework::mocks::ProposalBuilder;

        Ok(ProposalBuilder::default().build())
    }

    /// Unstable function to expose the zip317 interface for development
    // TODO: add correct functionality and doc comments / tests
    #[cfg(feature = "zip317")]
    pub async fn do_send_proposal(&self) -> Result<Vec<TxId>, String> {
        Ok(vec![TxId::from_bytes([0u8; 32])])
    }

    /// TODO: Add migrate_sapling_to_orchard argument
    pub async fn do_send(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<String, String> {
        let receivers = self.map_tos_to_receivers(address_amount_memo_tuples)?;
        let transaction_submission_height = self.get_submission_height().await?;
        // First, get the consensus branch ID
        debug!("Creating transaction");

        let result = {
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
        };

        result.map(|(transaction_id, _)| transaction_id)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_shield(
        &self,
        pools_to_shield: &[Pool],
        address: Option<String>,
    ) -> Result<String, String> {
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

        let addr = address
            .unwrap_or(self.wallet.wallet_capability().addresses()[0].encode(&self.config.chain));

        let receiver = self
            .map_tos_to_receivers(vec![(&addr, balance_to_shield - fee, None)])
            .expect("To build shield receiver.");
        let result = {
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
        };

        result.map(|(transaction_id, _)| transaction_id)
    }
}
