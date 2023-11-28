use super::*;

impl LightClient {
    pub async fn new_client_from_save_buffer(&self) -> Result<Self, ZingoLibError> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(ZingoLibError::CantReadWallet)
    }

    pub async fn do_send_permissive_policy(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<String, String> {
        let receivers = self.map_tos_to_receivers(address_amount_memo_tuples)?;
        let transaction_submission_height = self.get_submission_height().await?;
        // First, get the concensus branch ID
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
                    vec![
                        crate::wallet::Pool::Orchard,
                        crate::wallet::Pool::Sapling,
                        crate::wallet::Pool::Transparent,
                    ],
                    receivers,
                    transaction_submission_height,
                    |transaction_bytes| {
                        GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                    },
                )
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }
}
