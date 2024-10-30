//! This mod is mostly to take inputs, raw data amd comvert it into lightclient actions
//! (obvisouly) in a test environment.
use crate::{error::ZingoLibError, lightclient::LightClient};
use zcash_client_backend::{PoolType, ShieldedProtocol};

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
/// Get the total fees paid by a given client (assumes 1 capability per client).
pub async fn get_fees_paid_by_client(client: &LightClient) -> u64 {
    client.transaction_summaries().await.paid_fees()
}
/// Helpers to provide raw_receivers to lightclients for send and shield, etc.
pub mod from_inputs {

    use crate::lightclient::{send::send_with_proposal::QuickSendError, LightClient};

    /// Panics if the address, amount or memo conversion fails.
    pub async fn quick_send(
        quick_sender: &crate::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<nonempty::NonEmpty<zcash_primitives::transaction::TxId>, QuickSendError> {
        // TOdo fix expect
        let request = transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        quick_sender.quick_send(request).await
    }

    /// Panics if the address, amount or memo conversion fails.
    pub fn receivers_from_send_inputs(
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> crate::data::receivers::Receivers {
        raw_receivers
            .into_iter()
            .map(|(address, amount, memo)| {
                let recipient_address = crate::utils::conversion::address_from_str(address)
                    .expect("should be a valid address");
                let amount = crate::utils::conversion::zatoshis_from_u64(amount)
                    .expect("should be inside the range of valid zatoshis");
                let memo = memo.map(|memo| {
                    crate::wallet::utils::interpret_memo_string(memo.to_string())
                        .expect("should be able to interpret memo")
                });

                crate::data::receivers::Receiver::new(recipient_address, amount, memo)
            })
            .collect()
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from rust primitives for simplified test writing.
    pub fn transaction_request_from_send_inputs(
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<
        zcash_client_backend::zip321::TransactionRequest,
        zcash_client_backend::zip321::Zip321Error,
    > {
        let receivers = receivers_from_send_inputs(raw_receivers);
        crate::data::receivers::transaction_request_from_receivers(receivers)
    }

    /// Panics if the address, amount or memo conversion fails.
    pub async fn propose(
        proposer: &LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<
        crate::data::proposal::ProportionalFeeProposal,
        crate::wallet::propose::ProposeSendError,
    > {
        // TOdo fix expect
        let request = transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        proposer.propose_send(request).await
    }
}

/// gets stati for a vec of txids
pub async fn lookup_stati(
    client: &LightClient,
    txids: nonempty::NonEmpty<zcash_primitives::transaction::TxId>,
) -> nonempty::NonEmpty<zingo_status::confirmation_status::ConfirmationStatus> {
    let records = &client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .transaction_records_by_id;

    txids.map(|txid| records[&txid].status)
}
