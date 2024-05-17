//! As indicated by this file being behind the test-features flag, this is test-only functionality
//! In this context a "raw_receiver" is a 3 element stuple organizes primitives (e.g. from the command line)
//! into the components of a transaction receiver
//! raw_receiver.0:   A &str representing the receiver address
//! raw_receiver.1:   A u64 representing the number of zats to be sent
//! raw_receiver.2:   An Option<&str> that contains memo data if not None

use crate::{
    utils::conversion::{address_from_str, zatoshis_from_u64},
    wallet::Pool,
};

use crate::lightclient::LightClient;

use zingoconfig::ChainType;

use crate::data::receivers::Receivers;

/// Panics if the address, amount or memo conversion fails.
pub fn receivers_from_send_inputs(
    raw_receivers: Vec<(&str, u64, Option<&str>)>,
    chain: &ChainType,
) -> Receivers {
    raw_receivers
        .into_iter()
        .map(|(address, amount, memo)| {
            let recipient_address =
                address_from_str(address, chain).expect("should be a valid address");
            let amount =
                zatoshis_from_u64(amount).expect("should be inside the range of valid zatoshis");
            let memo = memo.map(|memo| {
                crate::wallet::utils::interpret_memo_string(memo.to_string())
                    .expect("should be able to interpret memo")
            });

            crate::data::receivers::Receiver::new(recipient_address, amount, memo)
        })
        .collect()
}

impl LightClient {
    /// Panics if the address, amount or memo conversion fails.
    pub async fn send_from_send_inputs(
        &self,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<String, String> {
        let receivers = receivers_from_send_inputs(raw_receivers, &self.config().chain);
        self.do_send(receivers).await.map(|txid| txid.to_string())
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
}
