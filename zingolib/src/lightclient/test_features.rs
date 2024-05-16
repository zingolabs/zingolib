use nonempty::NonEmpty;
use zcash_client_backend::{
    zip321::{TransactionRequest, Zip321Error},
    PoolType, ShieldedProtocol,
};
use zcash_primitives::transaction::TxId;

use crate::{
    data::{proposal::TransferProposal, receivers::transaction_request_from_receivers},
    error::ZingoLibError,
    utils::conversion::testing::receivers_from_send_inputs,
};

use super::{propose::ProposeSendError, send::send_with_proposal::QuickSendError, LightClient};

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn new_client_from_save_buffer(&self) -> Result<Self, ZingoLibError> {
        self.save_internal_buffer().await?;

        LightClient::read_wallet_from_buffer_async(
            &self.config,
            self.save_buffer.buffer.read().await.as_slice(),
        )
        .await
        .map_err(ZingoLibError::CantReadWallet)
    }

    /// Test only lightclient method for calling `propose_send` with primitive rust types
    ///
    /// # Panics
    ///
    /// Panics if the address, amount or memo conversion fails.
    pub async fn propose_send_from_send_inputs(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<TransferProposal, ProposeSendError> {
        let request = self
            .transaction_request_from_send_inputs(address_amount_memo_tuples)
            .expect("should be able to create a transaction request as receivers are valid.");
        self.propose_send(request).await
    }

    /// Test only lightclient method for calling `quick_send` with primitive rust types
    ///
    /// # Panics
    ///
    /// Panics if the address, amount or memo conversion fails.
    pub async fn quick_send_from_send_inputs(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<NonEmpty<TxId>, QuickSendError> {
        let request = self
            .transaction_request_from_send_inputs(address_amount_memo_tuples)
            .expect("should be able to create a transaction request as receivers are valid.");
        self.quick_send(request).await
    }

    /// gets the first address that will allow a sender to send to a specific pool, as a string
    pub async fn get_base_address(&self, pooltype: PoolType) -> String {
        match pooltype {
            PoolType::Transparent => self.do_addresses().await[0]["receivers"]["transparent"]
                .clone()
                .to_string(),
            PoolType::Shielded(ShieldedProtocol::Sapling) => self.do_addresses().await[0]
                ["receivers"]["sapling"]
                .clone()
                .to_string(),
            PoolType::Shielded(ShieldedProtocol::Orchard) => {
                self.do_addresses().await[0]["address"].take().to_string()
            }
        }
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from rust primitives for simplified test writing.
    pub fn transaction_request_from_send_inputs(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<TransactionRequest, Zip321Error> {
        let receivers =
            receivers_from_send_inputs(address_amount_memo_tuples, &self.config().chain);
        transaction_request_from_receivers(receivers)
    }
}
