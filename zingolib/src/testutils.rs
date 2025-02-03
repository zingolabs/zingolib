//! Zingo-Testutils
//! Holds functionality for zingo testing

#![warn(missing_docs)]

pub mod interrupts;
pub mod scenarios;

use crate::lightclient::describe::UAReceivers;
use crate::wallet::data::summaries::{
    OrchardNoteSummary, SaplingNoteSummary, SpendSummary, TransactionSummary,
    TransactionSummaryInterface as _, TransparentCoinSummary,
};
use crate::wallet::keys::unified::WalletCapability;
use crate::wallet::WalletBase;
use grpc_proxy::ProxyServer;
pub use incrementalmerkletree;
use std::cmp;
use std::collections::HashMap;
use std::io::Read;
use std::string::String;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use zcash_address::unified::Fvk;
use zcash_client_backend::{PoolType, ShieldedProtocol};

use crate::config::ZingoConfig;
use crate::lightclient::LightClient;
use json::JsonValue;
use log::debug;
use regtest::RegtestManager;
use tokio::time::sleep;

pub mod assertions;
pub mod chain_generics;
pub mod fee_tables;
/// TODO: Add Doc Comment Here!
pub mod grpc_proxy;
/// lightclient helpers
pub mod lightclient;
/// macros to help test
pub mod macros;
/// TODO: Add Doc Comment Here!
pub mod paths;
/// TODO: Add Doc Comment Here!
pub mod regtest;

/// TODO: Add Doc Comment Here!
pub fn build_fvks_from_wallet_capability(wallet_capability: &WalletCapability) -> [Fvk; 3] {
    let orchard_vk: orchard::keys::FullViewingKey =
        (&wallet_capability.unified_key_store).try_into().unwrap();
    let sapling_vk: sapling_crypto::zip32::DiversifiableFullViewingKey =
        (&wallet_capability.unified_key_store).try_into().unwrap();
    let transparent_vk: zcash_primitives::legacy::keys::AccountPubKey =
        (&wallet_capability.unified_key_store).try_into().unwrap();

    let mut transparent_vk_bytes = [0u8; 65];
    transparent_vk_bytes.copy_from_slice(&transparent_vk.serialize());

    [
        Fvk::Orchard(orchard_vk.to_bytes()),
        Fvk::Sapling(sapling_vk.to_bytes()),
        Fvk::P2pkh(transparent_vk_bytes),
    ]
}

/// TODO: Add Doc Comment Here!
pub async fn build_fvk_client(fvks: &[&Fvk], zingoconfig: &ZingoConfig) -> LightClient {
    let ufvk = zcash_address::unified::Encoding::encode(
        &<zcash_address::unified::Ufvk as zcash_address::unified::Encoding>::try_from_items(
            fvks.iter().copied().cloned().collect(),
        )
        .unwrap(),
        &zcash_address::Network::Regtest,
    );
    LightClient::create_unconnected(zingoconfig, WalletBase::Ufvk(ufvk), 0)
        .await
        .unwrap()
}
async fn get_synced_wallet_height(client: &LightClient) -> Result<u32, String> {
    client.do_sync(true).await?;
    Ok(client
        .do_wallet_last_scanned_height()
        .await
        .as_u32()
        .unwrap())
}

fn poll_server_height(manager: &RegtestManager) -> JsonValue {
    let temp_tips = manager.get_chain_tip().unwrap().stdout;
    let tips = json::parse(&String::from_utf8_lossy(&temp_tips)).unwrap();
    tips[0]["height"].clone()
}

/// TODO: Add Doc Comment Here!
/// This function _DOES NOT SYNC THE CLIENT/WALLET_.
pub async fn increase_server_height(manager: &RegtestManager, n: u32) {
    let start_height = poll_server_height(manager).as_fixed_point_u64(2).unwrap();
    let target = start_height + n as u64;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    let mut count = 0;
    while poll_server_height(manager).as_fixed_point_u64(2).unwrap() < target {
        sleep(Duration::from_millis(50)).await;
        count = dbg!(count + 1);
    }
}

/// TODO: doc comment
pub async fn assert_transaction_summary_exists(
    lightclient: &LightClient,
    expected: &TransactionSummary,
) {
    assert!(
        check_transaction_summary_exists(lightclient, expected).await,
        "wallet summaries: {}\n\n\nexpected: {}\n\n\n",
        lightclient.transaction_summaries().await,
        expected,
    );
}

/// TODO: doc comment
pub async fn check_transaction_summary_exists(
    lightclient: &LightClient,
    transaction_summary: &TransactionSummary,
) -> bool {
    lightclient
        .transaction_summaries()
        .await
        .iter()
        .any(|wallet_summary| {
            check_transaction_summary_equality(wallet_summary, transaction_summary)
        })
}

/// TODO: doc comment
pub fn assert_transaction_summary_equality(
    observed: &TransactionSummary,
    expected: &TransactionSummary,
) {
    assert!(
        check_transaction_summary_equality(observed, expected),
        "observed: {}\n\n\nexpected: {}\n\n\n",
        observed,
        expected,
    );
}

/// Transaction creation involves using a nonce, which means a non-deterministic txid.
/// Datetime is also based on time of run.
/// Check all the other fields
///   TODO:  seed random numbers in tests deterministically
pub fn check_transaction_summary_equality(
    first: &TransactionSummary,
    second: &TransactionSummary,
) -> bool {
    first.status() == second.status()
        && first.blockheight() == second.blockheight()
        && first.kind() == second.kind()
        && first.value() == second.value()
        && first.fee() == second.fee()
        && first.zec_price() == second.zec_price()
        && check_orchard_note_summary_equality(first.orchard_notes(), second.orchard_notes())
        && check_sapling_note_summary_equality(first.sapling_notes(), second.sapling_notes())
        && check_transparent_coin_summary_equality(
            first.transparent_coins(),
            second.transparent_coins(),
        )
        && first.outgoing_tx_data() == second.outgoing_tx_data()
}

/// TODO: doc comment
fn check_orchard_note_summary_equality(
    first: &[OrchardNoteSummary],
    second: &[OrchardNoteSummary],
) -> bool {
    if first.len() != second.len() {
        return false;
    };
    for i in 0..first.len() {
        if !(first[i].value() == second[i].value()
            && check_spend_status_equality(first[i].spend_summary(), second[i].spend_summary())
            && first[i].memo() == second[i].memo())
        {
            return false;
        }
    }
    true
}

/// TODO: doc comment
fn check_sapling_note_summary_equality(
    first: &[SaplingNoteSummary],
    second: &[SaplingNoteSummary],
) -> bool {
    if first.len() != second.len() {
        return false;
    };
    for i in 0..first.len() {
        if !(first[i].value() == second[i].value()
            && check_spend_status_equality(first[i].spend_summary(), second[i].spend_summary())
            && first[i].memo() == second[i].memo())
        {
            return false;
        }
    }
    true
}

/// TODO: doc comment
fn check_transparent_coin_summary_equality(
    first: &[TransparentCoinSummary],
    second: &[TransparentCoinSummary],
) -> bool {
    if first.len() != second.len() {
        return false;
    };
    for i in 0..first.len() {
        if !(first[i].value() == second[i].value()
            && check_spend_status_equality(first[i].spend_summary(), second[i].spend_summary()))
        {
            return false;
        }
    }
    true
}

fn check_spend_status_equality(first: SpendSummary, second: SpendSummary) -> bool {
    matches!(
        (first, second),
        (SpendSummary::Unspent, SpendSummary::Unspent)
            | (SpendSummary::Spent(_), SpendSummary::Spent(_))
            | (
                SpendSummary::TransmittedSpent(_),
                SpendSummary::TransmittedSpent(_)
            )
            | (SpendSummary::MempoolSpent(_), SpendSummary::MempoolSpent(_))
    )
}

/// Send from sender to recipient and then sync the recipient
pub async fn send_value_between_clients_and_sync(
    manager: &RegtestManager,
    sender: &LightClient,
    recipient: &LightClient,
    value: u64,
    address_type: &str,
) -> Result<String, String> {
    debug!(
        "recipient address is: {}",
        &recipient.do_addresses(UAReceivers::All).await[0]["address"]
    );
    let txid = lightclient::from_inputs::quick_send(
        sender,
        vec![(
            &crate::get_base_address_macro!(recipient, address_type),
            value,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(manager, sender, 1).await?;
    recipient.do_sync(false).await?;
    Ok(txid.first().to_string())
}

/// This function increases the chain height reliably (with polling) but
/// it _also_ ensures that the client state is synced.
/// Unsynced clients are very interesting to us.  See increate_server_height
/// to reliably increase the server without syncing the client
pub async fn increase_height_and_wait_for_client(
    manager: &RegtestManager,
    client: &LightClient,
    n: u32,
) -> Result<(), String> {
    wait_until_client_reaches_block_height(
        client,
        generate_n_blocks_return_new_height(manager, n)
            .await
            .expect("should find target height"),
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn generate_n_blocks_return_new_height(
    manager: &RegtestManager,
    n: u32,
) -> Result<u32, String> {
    let start_height = manager.get_current_height().unwrap();
    let target = start_height + n;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    assert_eq!(manager.get_current_height().unwrap(), target);
    Ok(target)
}

/// will hang if RegtestManager does not reach target_block_height
pub async fn wait_until_client_reaches_block_height(
    client: &LightClient,
    target_block_height: u32,
) -> Result<(), String> {
    while check_wallet_chainheight_value(client, target_block_height).await? {
        sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}
async fn check_wallet_chainheight_value(client: &LightClient, target: u32) -> Result<bool, String> {
    Ok(get_synced_wallet_height(client).await? != target)
}

/// TODO: Add Doc Comment Here!
pub struct RecordingReader<Reader> {
    from: Reader,
    read_lengths: Vec<usize>,
}
impl<T> Read for RecordingReader<T>
where
    T: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let for_info = self.from.read(buf)?;
        log::info!("{:?}", for_info);
        self.read_lengths.push(for_info);
        Ok(for_info)
    }
}

/// Number of notes created and consumed in a transaction.
#[derive(Debug)]
pub struct TxNotesCount {
    /// Transparent notes in transaction.
    pub transparent_tx_notes: usize,
    /// Sapling notes in transaction.
    pub sapling_tx_notes: usize,
    /// Orchard notes in transaction.
    pub orchard_tx_notes: usize,
}

/// Number of logical actions in a transaction
#[derive(Debug)]
pub struct TxActionsCount {
    /// Transparent actions in transaction
    pub transparent_tx_actions: usize,
    /// Sapling actions in transaction
    pub sapling_tx_actions: usize,
    /// Orchard notes in transaction
    pub orchard_tx_actions: usize,
}

/// Returns number of notes used as inputs for txid as TxNotesCount (transparent_notes, sapling_notes, orchard_notes).
pub async fn tx_inputs(client: &LightClient, txid: &str) -> TxNotesCount {
    let notes = client.do_list_notes(true).await;

    let mut transparent_notes = 0;
    let mut sapling_notes = 0;
    let mut orchard_notes = 0;

    if let JsonValue::Array(spent_utxos) = &notes["spent_utxos"] {
        for utxo in spent_utxos {
            if utxo["spent"] == txid || utxo["pending_spent"] == txid {
                transparent_notes += 1;
            }
        }
    }
    if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
        for utxo in pending_utxos {
            if utxo["spent"] == txid || utxo["pending_spent"] == txid {
                transparent_notes += 1;
            }
        }
    }

    if let JsonValue::Array(spent_sapling_notes) = &notes["spent_sapling_notes"] {
        for note in spent_sapling_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                sapling_notes += 1;
            }
        }
    }
    if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
        for note in pending_sapling_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                sapling_notes += 1;
            }
        }
    }

    if let JsonValue::Array(spent_orchard_notes) = &notes["spent_orchard_notes"] {
        for note in spent_orchard_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                orchard_notes += 1;
            }
        }
    }
    if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
        for note in pending_orchard_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                orchard_notes += 1;
            }
        }
    }

    TxNotesCount {
        transparent_tx_notes: transparent_notes,
        sapling_tx_notes: sapling_notes,
        orchard_tx_notes: orchard_notes,
    }
}

/// Returns number of notes created in txid as TxNotesCount (transparent_notes, sapling_notes, orchard_notes).
pub async fn tx_outputs(client: &LightClient, txid: &str) -> TxNotesCount {
    let notes = client.do_list_notes(true).await;

    let mut transparent_notes = 0;
    let mut sapling_notes = 0;
    let mut orchard_notes = 0;

    if let JsonValue::Array(unspent_utxos) = &notes["utxos"] {
        for utxo in unspent_utxos {
            if utxo["created_in_txid"] == txid {
                transparent_notes += 1;
            }
        }
    }

    if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
        for utxo in pending_utxos {
            if utxo["created_in_txid"] == txid {
                transparent_notes += 1;
            }
        }
    }

    if let JsonValue::Array(unspent_sapling_notes) = &notes["unspent_sapling_notes"] {
        for note in unspent_sapling_notes {
            if note["created_in_txid"] == txid {
                sapling_notes += 1;
            }
        }
    }

    if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
        for note in pending_sapling_notes {
            if note["created_in_txid"] == txid {
                sapling_notes += 1;
            }
        }
    }

    if let JsonValue::Array(unspent_orchard_notes) = &notes["unspent_orchard_notes"] {
        for note in unspent_orchard_notes {
            if note["created_in_txid"] == txid {
                orchard_notes += 1;
            }
        }
    }

    if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
        for note in pending_orchard_notes {
            if note["created_in_txid"] == txid {
                orchard_notes += 1;
            }
        }
    }

    TxNotesCount {
        transparent_tx_notes: transparent_notes,
        sapling_tx_notes: sapling_notes,
        orchard_tx_notes: orchard_notes,
    }
}

/// Returns total actions for txid as TxActionsCount.
pub async fn tx_actions(
    sender: &LightClient,
    recipient: Option<&LightClient>,
    txid: &str,
) -> TxActionsCount {
    let tx_ins = tx_inputs(sender, txid).await;
    let tx_outs = if let Some(rec) = recipient {
        tx_outputs(rec, txid).await
    } else {
        TxNotesCount {
            transparent_tx_notes: 0,
            sapling_tx_notes: 0,
            orchard_tx_notes: 0,
        }
    };
    let tx_change = tx_outputs(sender, txid).await;

    let calculated_sapling_tx_actions = cmp::max(
        tx_ins.sapling_tx_notes,
        tx_outs.sapling_tx_notes + tx_change.sapling_tx_notes,
    );
    let final_sapling_tx_actions = if calculated_sapling_tx_actions == 1 {
        2
    } else {
        calculated_sapling_tx_actions
    };

    let calculated_orchard_tx_actions = cmp::max(
        tx_ins.orchard_tx_notes,
        tx_outs.orchard_tx_notes + tx_change.orchard_tx_notes,
    );
    let final_orchard_tx_actions = if calculated_orchard_tx_actions == 1 {
        2
    } else {
        calculated_orchard_tx_actions
    };

    TxActionsCount {
        transparent_tx_actions: cmp::max(
            tx_ins.transparent_tx_notes,
            tx_outs.transparent_tx_notes + tx_change.transparent_tx_notes,
        ),
        sapling_tx_actions: final_sapling_tx_actions,
        orchard_tx_actions: final_orchard_tx_actions,
    }
}

/// Returns the total transfer value of txid.
pub async fn total_tx_value(client: &LightClient, txid: &str) -> u64 {
    let notes = client.do_list_notes(true).await;

    let mut tx_spend: u64 = 0;
    let mut tx_change: u64 = 0;
    if let JsonValue::Array(spent_utxos) = &notes["spent_utxos"] {
        for utxo in spent_utxos {
            if utxo["spent"] == txid || utxo["pending_spent"] == txid {
                tx_spend += utxo["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
        for utxo in pending_utxos {
            if utxo["spent"] == txid || utxo["pending_spent"] == txid {
                tx_spend += utxo["value"].as_u64().unwrap();
            } else if utxo["created_in_txid"] == txid {
                tx_change += utxo["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(unspent_utxos) = &notes["utxos"] {
        for utxo in unspent_utxos {
            if utxo["created_in_txid"] == txid {
                tx_change += utxo["value"].as_u64().unwrap();
            }
        }
    }

    if let JsonValue::Array(spent_sapling_notes) = &notes["spent_sapling_notes"] {
        for note in spent_sapling_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                tx_spend += note["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
        for note in pending_sapling_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                tx_spend += note["value"].as_u64().unwrap();
            } else if note["created_in_txid"] == txid {
                tx_change += note["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(unspent_sapling_notes) = &notes["unspent_sapling_notes"] {
        for note in unspent_sapling_notes {
            if note["created_in_txid"] == txid {
                tx_change += note["value"].as_u64().unwrap();
            }
        }
    }

    if let JsonValue::Array(spent_orchard_notes) = &notes["spent_orchard_notes"] {
        for note in spent_orchard_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                tx_spend += note["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
        for note in pending_orchard_notes {
            if note["spent"] == txid || note["pending_spent"] == txid {
                tx_spend += note["value"].as_u64().unwrap();
            } else if note["created_in_txid"] == txid {
                tx_change += note["value"].as_u64().unwrap();
            }
        }
    }
    if let JsonValue::Array(unspent_orchard_notes) = &notes["unspent_orchard_notes"] {
        for note in unspent_orchard_notes {
            if note["created_in_txid"] == txid {
                tx_change += note["value"].as_u64().unwrap();
            }
        }
    }

    tx_spend - tx_change
}

/// TODO: Add Doc Comment Here!
#[allow(clippy::type_complexity)]
pub fn start_proxy_and_connect_lightclient(
    client: &LightClient,
    conditional_operations: HashMap<&'static str, Box<dyn Fn(Arc<AtomicBool>) + Send + Sync>>,
) -> (
    JoinHandle<Result<(), tonic::transport::Error>>,
    Arc<AtomicBool>,
) {
    let proxy_online = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let proxy_port = portpicker::pick_unused_port().unwrap();
    let proxy_uri = format!("http://localhost:{proxy_port}");
    let proxy_handle = ProxyServer {
        lightwalletd_uri: client.get_server_uri(),
        online: proxy_online.clone(),
        conditional_operations,
    }
    .serve(proxy_port);
    client.set_server(proxy_uri.parse().unwrap());
    (proxy_handle, proxy_online)
}

/// TODO: Add Doc Comment Here!
pub async fn check_proxy_server_works() {
    let (_regtest_manager, _cph, ref faucet) = scenarios::faucet_default().await;
    let (_proxy_handle, proxy_status) = start_proxy_and_connect_lightclient(faucet, HashMap::new());
    proxy_status.store(false, std::sync::atomic::Ordering::Relaxed);
    tokio::task::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        println!("Wakening proxy!");
        proxy_status.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    println!("Doing info!");
    println!("{}", faucet.do_info().await)
}

/// TODO: Add Doc Comment Here!
pub fn port_to_localhost_uri(port: impl std::fmt::Display) -> http::Uri {
    format!("http://localhost:{port}").parse().unwrap()
}

/// a quick and dirty way to proptest across protocols.
pub fn int_to_shieldedprotocol(int: i32) -> ShieldedProtocol {
    match int {
        1 => ShieldedProtocol::Sapling,
        2 => ShieldedProtocol::Orchard,
        _ => panic!("invalid protocol"),
    }
}

/// a quick and dirty way to proptest across pools.
pub fn int_to_pooltype(int: i32) -> PoolType {
    match int {
        0 => PoolType::Transparent,
        n => PoolType::Shielded(int_to_shieldedprotocol(n)),
    }
}
