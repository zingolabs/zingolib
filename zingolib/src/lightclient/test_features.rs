use super::*;

impl LightClient {
    pub async fn new_client_from_save_buffer(&self) -> crate::error::ZingoLibResult<Self> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(crate::error::ZingoLibError::CantReadWallet)
    }
}
