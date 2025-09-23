//! This mod is mostly to take inputs, raw data amd convert it into lightclient actions
//! (obviously) in a test environment.

use crate::wallet::balance::AccountBalance;
use zcash_primitives::transaction::TxId;
use zcash_protocol::{PoolType, ShieldedProtocol};
use zip32::AccountId;

use crate::{
    lightclient::{LightClient, error::LightClientError},
    wallet::LightWallet,
};

/// Create a lightclient from the buffer of another
pub async fn new_client_from_save_buffer(
    template_client: &mut LightClient,
) -> Result<LightClient, LightClientError> {
    let mut wallet_bytes: Vec<u8> = vec![];
    template_client
        .wallet
        .write()
        .await
        .write(&mut wallet_bytes, &template_client.config.chain)?;

    LightClient::create_from_wallet(
        LightWallet::read(wallet_bytes.as_slice(), template_client.config.chain)?,
        template_client.config.clone(),
        false,
    )
}
/// gets the first address that will allow a sender to send to a specific pool, as a string
pub async fn get_base_address(client: &LightClient, pooltype: PoolType) -> String {
    match pooltype {
        PoolType::Shielded(ShieldedProtocol::Orchard) => {
            assert!(
                client.unified_addresses_json().await[0]["has_orchard"]
                    .as_bool()
                    .unwrap()
            );
            client.unified_addresses_json().await[0]["encoded_address"]
                .clone()
                .to_string()
        }
        PoolType::Shielded(ShieldedProtocol::Sapling) => {
            assert!(
                !client.unified_addresses_json().await[1]["has_orchard"]
                    .as_bool()
                    .unwrap()
            );
            assert!(
                client.unified_addresses_json().await[1]["has_sapling"]
                    .as_bool()
                    .unwrap()
            );
            client.unified_addresses_json().await[1]["encoded_address"]
                .clone()
                .to_string()
        }
        PoolType::Transparent => client.transparent_addresses_json().await[0]["encoded_address"]
            .clone()
            .to_string(),
    }
}
/// Get the total fees paid by a given client (assumes 1 capability per client).
pub async fn get_fees_paid_by_client(client: &LightClient) -> u64 {
    client
        .transaction_summaries(false)
        .await
        .unwrap()
        .paid_fees()
}
/// Helpers to provide raw_receivers to lightclients for send and shield, etc.
pub mod from_inputs {

    use nonempty::NonEmpty;
    use zcash_primitives::transaction::TxId;

    use crate::{
        lightclient::{LightClient, error::QuickSendError},
        wallet::error::ProposeSendError,
    };

    /// Panics if the address, amount or memo conversion fails.
    pub async fn quick_send(
        quick_sender: &mut crate::lightclient::LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<NonEmpty<TxId>, QuickSendError> {
        let request = transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        quick_sender
            .quick_send(request, zip32::AccountId::ZERO)
            .await
    }

    /// Panics if the address, amount or memo conversion fails.
    pub(crate) fn receivers_from_send_inputs(
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
        proposer: &mut LightClient,
        raw_receivers: Vec<(&str, u64, Option<&str>)>,
    ) -> Result<crate::data::proposal::ProportionalFeeProposal, ProposeSendError> {
        let request = transaction_request_from_send_inputs(raw_receivers)
            .expect("should be able to create a transaction request as receivers are valid.");
        proposer.propose_send(request, zip32::AccountId::ZERO).await
    }
}

/// gets stati for a vec of txids
pub async fn lookup_statuses(
    client: &LightClient,
    txids: nonempty::NonEmpty<TxId>,
) -> nonempty::NonEmpty<Option<zingo_status::confirmation_status::ConfirmationStatus>> {
    let wallet = client.wallet.read().await;

    txids.map(|txid| {
        wallet
            .wallet_transactions
            .get(&txid)
            .map(|transaction_record| transaction_record.status())
    })
}

/// TODO: Add Doc Comment Here!
// TODO: move balance fns to wallet balance sub-module and also move this struct there
#[deprecated = "pita interface"]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolBalances {
    /// TODO: Add Doc Comment Here!
    pub sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub verified_sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub spendable_sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub unverified_sapling_balance: Option<u64>,

    /// TODO: Add Doc Comment Here!
    pub orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub verified_orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub unverified_orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub spendable_orchard_balance: Option<u64>,

    /// TODO: Add Doc Comment Here!
    pub confirmed_transparent_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub unconfirmed_transparent_balance: Option<u64>,
}

impl LightClient {
    #[deprecated = "pita interface"]
    pub async fn do_balance(&self) -> PoolBalances {
        let AccountBalance {
            confirmed_orchard_balance,
            unconfirmed_orchard_balance,
            total_orchard_balance,

            confirmed_sapling_balance,
            unconfirmed_sapling_balance,
            total_sapling_balance,

            confirmed_transparent_balance,
            unconfirmed_transparent_balance,
            total_transparent_balance,
        } = self.account_balance(AccountId::ZERO).await.unwrap();

        PoolBalances {
            sapling_balance: total_sapling_balance.map(|zatoshis| zatoshis.into_u64()),
            verified_sapling_balance: confirmed_sapling_balance.map(|zatoshis| zatoshis.into_u64()),
            spendable_sapling_balance: None,
            unverified_sapling_balance: unconfirmed_sapling_balance
                .map(|zatoshis| zatoshis.into_u64()),

            orchard_balance: total_orchard_balance.map(|zatoshis| zatoshis.into_u64()),
            verified_orchard_balance: confirmed_orchard_balance.map(|zatoshis| zatoshis.into_u64()),
            spendable_orchard_balance: None,
            unverified_orchard_balance: unconfirmed_orchard_balance
                .map(|zatoshis| zatoshis.into_u64()),

            confirmed_transparent_balance: confirmed_transparent_balance
                .map(|zatoshis| zatoshis.into_u64()),
            unconfirmed_transparent_balance: unconfirmed_transparent_balance
                .map(|zatoshis| zatoshis.into_u64()),
        }
    }
}
