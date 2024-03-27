use crate::{
    error::ZingoLibError,
    wallet::{
        send::{build_transaction_request_from_tuples, errors::DoProposeError},
        transactions::Proposa,
        Pool, SendProgress,
    },
};

use json::{object, JsonValue};
use log::error;

use zcash_primitives::{memo::MemoBytes, transaction::components::amount::NonNegativeAmount};
use zcash_proofs::prover::LocalTxProver;

use super::LightClient;

static LOG_INIT: std::sync::Once = std::sync::Once::new();

const MARGINAL_FEE: u64 = 5_000; // From ZIP-317

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
    ) -> Result<Proposa, DoProposeError> {
        let request =
            build_transaction_request_from_tuples(self.config.chain, address_amount_memo_tuples)
                .map_err(|e| DoProposeError::RequestConstruction(e))?;

        let _lock = self.sync_lock.lock().await;

        self.wallet
            .propose_transfer(request)
            .await
            .map_err(|e| DoProposeError::Proposing(e))
    }

    pub async fn do_send_proposal(&self) -> Result<String, String> {
        let (sapling_output, sapling_spend) = self.read_sapling_params()?;
        let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let transaction_submission_height = self.get_submission_height().await?;

        let _result = {
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
        };

        // result.map_err(|e| e.to_string())
        Ok("todo".to_string())
    }

    pub async fn do_send(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<String, String> {
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
    ) -> Result<String, String> {
        //     let transaction_submission_height = self.get_submission_height().await?;
        //     let fee = u64::from(MINIMUM_FEE); // TODO: This can no longer be hard coded, and must be calced
        //                                       // as a fn of the transactions structure.
        //     let tbal = self
        //         .wallet
        //         .tbalance(None)
        //         .await
        //         .expect("to receive a balance");
        //     let sapling_bal = self
        //         .wallet
        //         .spendable_sapling_balance(None)
        //         .await
        //         .unwrap_or(0);

        //     // Make sure there is a balance, and it is greater than the amount
        //     let balance_to_shield = if pools_to_shield.contains(&Pool::Transparent) {
        //         tbal
        //     } else {
        //         0
        //     } + if pools_to_shield.contains(&Pool::Sapling) {
        //         sapling_bal
        //     } else {
        //         0
        //     };
        //     if balance_to_shield <= fee {
        //         return Err(format!(
        //             "Not enough transparent/sapling balance to shield. Have {} zats, need more than {} zats to cover tx fee",
        //             balance_to_shield, fee
        //         ));
        //     }

        //     let addr = address
        //         .unwrap_or(self.wallet.wallet_capability().addresses()[0].encode(&self.config.chain));

        //     let receiver = self.map_tos_to_receivers(vec![(&addr, balance_to_shield - fee, None)])?;
        //     let request = build_transaction_request_from_receivers(receiver)
        //         .map_err(|e| ZingoLibError::RequestConstruction(e).to_string())?;
        //     let result = {
        //         let _lock = self.sync_lock.lock().await;
        //         let (sapling_output, sapling_spend) = self.read_sapling_params()?;

        //         let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        //         self.wallet
        //             .send_to_addresses(
        //                 sapling_prover,
        //                 pools_to_shield.to_vec(),
        //                 request,
        //                 transaction_submission_height,
        //                 |transaction_bytes| {
        //                     crate::grpc_connector::send_transaction(
        //                         self.get_server_uri(),
        //                         transaction_bytes,
        //                     )
        //                 },
        //             )
        //             .await
        //     };

        //     result.map_err(|e| e.to_string())
        Ok("todo".to_string())
    }
}
