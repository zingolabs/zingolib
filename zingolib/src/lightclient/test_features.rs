//! As indicated by this file being behind the test-features flag, this is test-only functionality
//! In this context a "raw_receiver" is a 3 element stuple organizes primitives (e.g. from the command line)
//! into the components of a transaction receiver
//! raw_receiver.0:   A &str representing the receiver address
//! raw_receiver.1:   A u64 representing the number of zats to be sent
//! raw_receiver.2:   An Option<&str> that contains memo data if not None
use nonempty::NonEmpty;
use zcash_client_backend::{
    zip321::{TransactionRequest, Zip321Error},
    PoolType, ShieldedProtocol,
};
use zcash_primitives::transaction::TxId;

use crate::{
    data::{proposal::TransferProposal, receivers::transaction_request_from_receivers},
    error::ZingoLibError,
    utils::conversion::{address_from_str, testing::receivers_from_send_inputs},
    wallet::Pool,
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

    /// Panics if the address, amount or memo conversion fails.
    pub async fn propose_send_from_send_inputs(
        &self,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<TransferProposal, ProposeSendError> {
        let request = self
            .transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        self.propose_send(request).await
    }

    /// Panics if the address, amount or memo conversion fails.
    pub async fn quick_send_from_send_inputs(
        &self,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<NonEmpty<TxId>, QuickSendError> {
        let request = self
            .transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        self.quick_send(request).await
    }

    /// Panics if the address, amount or memo conversion fails.
    pub async fn send_from_send_inputs(
        &self,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        let receivers = receivers_from_send_inputs(raw_receivers, &self.config().chain);
        self.do_send(receivers).await.map(|txid| txid.to_string())
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from rust primitives for simplified test writing.
    pub fn transaction_request_from_send_inputs(
        &self,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<TransactionRequest, Zip321Error> {
        let receivers = receivers_from_send_inputs(raw_receivers, &self.config().chain);
        transaction_request_from_receivers(receivers)
    }

    /// Panics if the address conversion fails.
    pub async fn shield_from_shield_inputs(
        &self,
        pools_to_shield: &[Pool],
        address: Option<&str>,
    ) -> Result<String, String> {
        let address = address.map(|addr| {
            address_from_str(addr, &self.config().chain).expect("should be a valid address")
        });
        self.do_shield(pools_to_shield, address)
            .await
            .map(|txid| txid.to_string())
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
}
