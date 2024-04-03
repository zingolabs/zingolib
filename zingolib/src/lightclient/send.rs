//! The sequence of steps necessary to send a zingo transaction are as follows:
//!
//!  (1) create a proposed transaction
//!
//!  (2) request that Thor bless the proposal with a thunderbolt
//!
//!  (3) tie the proposal to a European swallow
//!
//!  (4) slap the swallow on the ass and yell:  Hee-aw!
use crate::{
    error::ZingoLibError,
    wallet::{
        send::{build_transaction_request_from_tuples, errors::DoProposeError},
        transactions::{Proposa, TransferProposal, TxMapAndMaybeTrees},
        Pool, SendProgress,
    },
};

use json::{object, JsonValue};
use log::error;

use tokio::sync::RwLockWriteGuard;
use zcash_primitives::{
    memo::MemoBytes,
    transaction::{components::amount::NonNegativeAmount, TxId},
};
use zcash_proofs::prover::LocalTxProver;

use super::LightClient;

static LOG_INIT: std::sync::Once = std::sync::Once::new();

#[derive(Debug, Clone)]
pub struct LightWalletSendProgress {
    pub progress: SendProgress,
    pub interrupt_sync: bool,
}

impl LightWalletSendProgress {
    pub fn to_json(&self) -> JsonValue {
        object! {
            "id" => self.progress.id,
            "sending" => self.progress.is_send_in_progress,
            "progress" => self.progress.progress,
            "total" => self.progress.total,
            "txid" => self.progress.last_transaction_id.clone(),
            "error" => self.progress.last_error.clone(),
            "sync_interrupt" => self.interrupt_sync
        }
    }
}

impl LightClient {
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

    pub async fn do_propose(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<TransferProposal, DoProposeError> {
        let request =
            build_transaction_request_from_tuples(self.config.chain, address_amount_memo_tuples)
                .map_err(|e| DoProposeError::RequestConstruction(e))?;

        let _lock = self.sync_lock.lock().await;

        self.wallet
            .propose_transfer(request)
            .await
            .map_err(|e| DoProposeError::Proposing(e))
    }

    pub async fn do_send_proposal(&self) -> Result<Vec<TxId>, String> {
        let (sapling_output, sapling_spend) = self.read_sapling_params()?;
        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let transaction_submission_height = self.get_submission_height().await?;

        self.wallet
            .send_proposed_transfer(
                transaction_submission_height,
                |transaction_bytes| {
                    crate::grpc_connector::send_transaction(
                        self.get_server_uri(),
                        transaction_bytes,
                    )
                },
                sapling_prover,
            )
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn do_send(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<Vec<TxId>, String> {
        self.do_propose(address_amount_memo_tuples)
            .await
            .map_err(|e| e.to_string())?;
        self.do_send_proposal().await
    }

    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
        })
    }

    pub async fn do_shield(
        &self,
        _pools_to_shield: &[Pool],
        _address: Option<String>,
    ) -> Result<Vec<TxId>, String> {
        let mut context_write_lock: RwLockWriteGuard<'_, TxMapAndMaybeTrees> = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        let mut spend_kit = self
            .wallet
            .assemble_spend_kit(&mut context_write_lock)
            .await?;

        spend_kit.propose_shielding()?;
        std::mem::drop(spend_kit);
        self.do_send_proposal().await
    }
}
