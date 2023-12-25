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
    pub async fn do_list_txsummaries_assert(&self) -> Vec<ValueTransfer> {
        let lts = self.list_tx_summaries().await;
        assert_eq!(lts.1.len(), 0);
        lts.0
    }
}
