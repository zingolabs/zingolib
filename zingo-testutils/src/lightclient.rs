//! This mod is mostly to take inputs, raw data amd comvert it into lightclient actions
//! (obvisouly) in a test environment.
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zingolib::{error::ZingoLibError, lightclient::LightClient};

/// Create a lightclient from the buffer of another
pub async fn new_client_from_save_buffer(
    template_client: &LightClient,
) -> Result<LightClient, ZingoLibError> {
    let buffer = template_client.save_internal_buffer().await?;

    LightClient::read_wallet_from_buffer_async(template_client.config(), buffer.as_slice())
        .await
        .map_err(ZingoLibError::CantReadWallet)
}
/// gets the first address that will allow a sender to send to a specific pool, as a string
/// calling \[0] on json may panic? not sure -fv
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
/// Helpers to provide raw_receivers to lightclients for send and shield, etc.
pub mod from_inputs {
    use zcash_client_backend::PoolType;
    use zingolib::lightclient::{send::send_with_proposal::QuickSendError, LightClient};

    /// Panics if the address, amount or memo conversion fails.
    pub async fn quick_send(
        quick_sender: &zingolib::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<nonempty::NonEmpty<zcash_primitives::transaction::TxId>, QuickSendError> {
        let request = transaction_request_from_send_inputs(quick_sender, raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        quick_sender.quick_send(request).await
    }

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

    /// In a test give sender a raw_receiver to encode and send to
    pub async fn old_send(
        sender: &zingolib::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        let receivers = receivers_from_send_inputs(raw_receivers, &sender.config().chain);
        sender.do_send(receivers).await.map(|txid| txid.to_string())
    }

    /// Panics if the address conversion fails.
    pub async fn shield(
        shielder: &LightClient,
        pools_to_shield: &[PoolType],
        address: Option<&str>,
    ) -> Result<String, String> {
        let address = address.map(|addr| {
            zingolib::utils::conversion::address_from_str(addr, &shielder.config().chain)
                .expect("should be a valid address")
        });
        shielder
            .do_shield(pools_to_shield, address)
            .await
            .map(|txid| txid.to_string())
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

    /// Panics if the address, amount or memo conversion fails.
    pub async fn propose(
        proposer: &LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<
        zingolib::data::proposal::TransferProposal,
        zingolib::lightclient::propose::ProposeSendError,
    > {
        let request = transaction_request_from_send_inputs(proposer, raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        proposer.propose_send(request).await
    }
}

pub mod with_assertions;
