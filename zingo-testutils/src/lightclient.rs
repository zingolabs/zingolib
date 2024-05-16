use zingolib::{error::ZingoLibError, lightclient::LightClient};

/// Create a lightclient from the buffer of another
pub async fn new_client_from_save_buffer(
    template_client: &LightClient,
) -> Result<LightClient, ZingoLibError> {
    let buffer = template_client.save_internal_buffer().await?;

    LightClient::read_wallet_from_buffer_async(&template_client.config(), buffer.as_slice())
        .await
        .map_err(ZingoLibError::CantReadWallet)
}
