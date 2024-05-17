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
mod from_inputs {
    /// Panics if the address, amount or memo conversion fails.
    pub fn receivers_from_send_inputs(
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
        chain: &zingoconfig::ChainType,
    ) -> zingolib::data::receivers::Receivers {
        raw_receivers
            .into_iter()
            .map(|(address, amount, memo)| {
                let recipient_address =
                    zingolib::utils::conversion::address_from_str(address, chain)
                        .expect("should be a valid address");
                let amount = zingolib::utils::conversion::zatoshis_from_u64(amount)
                    .expect("should be inside the range of valid zatoshis");
                let memo = memo.map(|memo| {
                    zingolib::wallet::utils::interpret_memo_string(memo.to_string())
                        .expect("should be able to interpret memo")
                });

                zingolib::data::receivers::Receiver::new(recipient_address, amount, memo)
            })
            .collect()
    }
    pub async fn send_from_send_inputs(
        sender: &zingolib::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        let receivers = receivers_from_send_inputs(raw_receivers, &sender.config().chain);
        sender.do_send(receivers).await.map(|txid| txid.to_string())
    }
}
