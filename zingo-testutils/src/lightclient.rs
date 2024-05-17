use zcash_client_backend::{PoolType, ShieldedProtocol};
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
/// gets the first address that will allow a sender to send to a specific pool, as a string
pub async fn get_base_address(client: &LightClient, pooltype: PoolType) -> String {
    match pooltype {
        PoolType::Transparent => client.do_addresses().await[0]["receivers"]["transparent"]
            .clone()
            .to_string(),
        PoolType::Shielded(ShieldedProtocol::Sapling) => client.do_addresses().await[0]
            ["receivers"]["sapling"]
            .clone()
            .to_string(),
        PoolType::Shielded(ShieldedProtocol::Orchard) => {
            client.do_addresses().await[0]["address"].take().to_string()
        }
    }
}
pub(crate) mod from_inputs {
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

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from rust primitives for simplified test writing.
    pub fn transaction_request_from_send_inputs(
        requester: &zingolib::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<
        zcash_client_backend::zip321::TransactionRequest,
        zcash_client_backend::zip321::Zip321Error,
    > {
        let receivers = receivers_from_send_inputs(raw_receivers, &requester.config().chain);
        zingolib::data::receivers::transaction_request_from_receivers(receivers)
    }
}
