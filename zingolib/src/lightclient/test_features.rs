use zcash_client_backend::{PoolType, ShieldedProtocol};

use crate::{
    data::{proposal::TransferProposal, receivers::transaction_request_from_receivers},
    error::ZingoLibError,
    utils::conversion::{address_from_str, testing::receivers_from_send_inputs},
    wallet::Pool,
};

use super::{propose::ProposeSendError, *};

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

    /// Test only lightclient method for calling `do_propose` with primitive rust types
    ///
    /// # Panics
    ///
    /// Panics if the address, amount or memo conversion fails.
    pub async fn propose_send_test_only(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<TransferProposal, ProposeSendError> {
        let receivers =
            receivers_from_send_inputs(address_amount_memo_tuples, &self.config().chain);
        let request = transaction_request_from_receivers(receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        self.propose_send(request).await
    }

    /// Test only lightclient method for calling `do_send` with primitive rust types
    ///
    /// # Panics
    ///
    /// Panics if the address, amount or memo conversion fails.
    pub async fn send_test_only(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        let receivers =
            receivers_from_send_inputs(address_amount_memo_tuples, &self.config().chain);
        self.do_send(receivers).await.map(|txid| txid.to_string())
    }

    /// Test only lightclient method for calling `do_shield` with an address as &str
    ///
    /// # Panics
    ///
    /// Panics if the address conversion fails.
    pub async fn shield_test_only(
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
