use zcash_client_backend::proposal::{Proposal, Step};
use zcash_primitives::transaction::fees::zip317::FeeRule;

use super::*;

impl LightClient {
    pub async fn new_client_from_save_buffer(&self) -> ZingoLibResult<Self> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(ZingoLibError::CantReadWallet)
    }
}
