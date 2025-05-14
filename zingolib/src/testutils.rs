//! Zingo-Testutils
//! Holds functionality for zingo testing

#![warn(missing_docs)]

pub mod scenarios;

use std::{io::Read, string::String, time::Duration};

use json::JsonValue;

use pepper_sync::sync::{SyncConfig, TransparentAddressDiscovery};
use zcash_address::unified::Fvk;
use zcash_protocol::{PoolType, ShieldedProtocol};

use crate::config::ZingoConfig;
use crate::lightclient::LightClient;
use crate::lightclient::error::LightClientError;
use crate::wallet::data::summaries::{
    BasicCoinSummary, BasicNoteSummary, OutgoingNoteSummary, TransactionSummary,
    TransactionSummaryInterface as _,
};
use crate::wallet::keys::unified::UnifiedKeyStore;
use crate::wallet::output::SpendStatus;
use crate::wallet::{LightWallet, WalletBase, WalletSettings};
use lightclient::get_base_address;
use regtest::RegtestManager;

pub mod assertions;
pub mod chain_generics;
pub mod fee_tables;
/// lightclient helpers
pub mod lightclient;
/// macros to help test
pub mod macros;
/// TODO: Add Doc Comment Here!
pub mod paths;
/// TODO: Add Doc Comment Here!
pub mod regtest;

/// TODO: Add Doc Comment Here!
pub fn build_fvks_from_unified_keystore(unified_keystore: &UnifiedKeyStore) -> [Fvk; 3] {
    let orchard_vk: orchard::keys::FullViewingKey = unified_keystore.try_into().unwrap();
    let sapling_vk: sapling_crypto::zip32::DiversifiableFullViewingKey =
        unified_keystore.try_into().unwrap();
    let transparent_vk: zcash_primitives::legacy::keys::AccountPubKey =
        unified_keystore.try_into().unwrap();

    let mut transparent_vk_bytes = [0u8; 65];
    transparent_vk_bytes.copy_from_slice(&transparent_vk.serialize());

    [
        Fvk::Orchard(orchard_vk.to_bytes()),
        Fvk::Sapling(sapling_vk.to_bytes()),
        Fvk::P2pkh(transparent_vk_bytes),
    ]
}

/// TODO: Add Doc Comment Here!
pub fn build_fvk_client(fvks: &[&Fvk], config: ZingoConfig) -> LightClient {
    let ufvk = zcash_address::unified::Encoding::encode(
        &<zcash_address::unified::Ufvk as zcash_address::unified::Encoding>::try_from_items(
            fvks.iter().copied().cloned().collect(),
        )
        .unwrap(),
        &zcash_protocol::consensus::NetworkType::Regtest,
    );
    LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::Ufvk(ufvk),
            0.into(),
            WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                },
            },
        )
        .unwrap(),
        config,
        false,
    )
    .unwrap()
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
    while poll_server_height(manager).as_fixed_point_u64(2).unwrap() < target {
        tokio::time::sleep(Duration::from_millis(50)).await;
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
        lightclient.transaction_summaries().await.unwrap(),
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
        .unwrap()
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
        && check_note_summary_equality(first.orchard_notes(), second.orchard_notes())
        && check_note_summary_equality(first.sapling_notes(), second.sapling_notes())
        && check_transparent_coin_summary_equality(
            first.transparent_coins(),
            second.transparent_coins(),
        )
        && check_outgoing_note_summary_equality(
            first.outgoing_orchard_notes(),
            second.outgoing_orchard_notes(),
        )
        && check_outgoing_note_summary_equality(
            first.outgoing_sapling_notes(),
            second.outgoing_sapling_notes(),
        )
}

fn check_note_summary_equality(first: &[BasicNoteSummary], second: &[BasicNoteSummary]) -> bool {
    if first.len() != second.len() {
        return false;
    };
    for i in 0..first.len() {
        if !(first[i].value() == second[i].value()
            && check_spend_status_equality(first[i].spend_status(), second[i].spend_status())
            && first[i].memo() == second[i].memo())
        {
            return false;
        }
    }
    true
}

fn check_outgoing_note_summary_equality(
    first: &[OutgoingNoteSummary],
    second: &[OutgoingNoteSummary],
) -> bool {
    if first.len() != second.len() {
        return false;
    };
    for i in 0..first.len() {
        if !(first[i].value == second[i].value
            && first[i].memo == second[i].memo
            && first[i].recipient == second[i].recipient
            && first[i].recipient_unified_address == second[i].recipient_unified_address)
            && first[i].account_id == second[i].account_id
            && first[i].scope == second[i].scope
        {
            return false;
        }
    }
    true
}

/// TODO: doc comment
fn check_transparent_coin_summary_equality(
    first: &[BasicCoinSummary],
    second: &[BasicCoinSummary],
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

fn check_spend_status_equality(first: SpendStatus, second: SpendStatus) -> bool {
    matches!(
        (first, second),
        (SpendStatus::Unspent, SpendStatus::Unspent)
            | (SpendStatus::Spent(_), SpendStatus::Spent(_))
            | (
                SpendStatus::TransmittedSpent(_),
                SpendStatus::TransmittedSpent(_)
            )
            | (SpendStatus::MempoolSpent(_), SpendStatus::MempoolSpent(_))
    )
}

/// Send from sender to recipient and then bump chain and sync both lightclients
pub async fn send_value_between_clients_and_sync(
    manager: &RegtestManager,
    sender: &mut LightClient,
    recipient: &mut LightClient,
    value: u64,
    address_pool: PoolType,
) -> Result<String, LightClientError> {
    let txid = lightclient::from_inputs::quick_send(
        sender,
        vec![(
            &get_base_address(recipient, address_pool).await,
            value,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(manager, sender, 1).await?;
    recipient.sync_and_await().await?;
    Ok(txid.first().to_string())
}

/// This function increases the chain height reliably (with polling) but
/// it _also_ ensures that the client state is synced.
/// Unsynced clients are very interesting to us.  See increase_server_height
/// to reliably increase the server without syncing the client
pub async fn increase_height_and_wait_for_client(
    manager: &RegtestManager,
    client: &mut LightClient,
    n: u32,
) -> Result<(), LightClientError> {
    sync_to_target_height(
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

/// Will hang if chain does not reach `target_block_height`
pub async fn sync_to_target_height(
    client: &mut LightClient,
    target_block_height: u32,
) -> Result<(), LightClientError> {
    // sync first so ranges exist for the `fully_scanned_height` call
    client.sync_and_await().await?;
    while u32::from(
        client
            .wallet
            .lock()
            .await
            .sync_state
            .fully_scanned_height()
            .unwrap(),
    ) < target_block_height
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
        client.sync_and_await().await?;
    }
    Ok(())
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

// FIXME: zingo2 rewrite with wallet data or note summaries
// /// Returns number of notes used as inputs for txid as TxNotesCount (transparent_notes, sapling_notes, orchard_notes).
// pub async fn tx_inputs(client: &LightClient, txid: &str) -> TxNotesCount {
//     let notes = client.do_list_notes(true).await;

//     let mut transparent_notes = 0;
//     let mut sapling_notes = 0;
//     let mut orchard_notes = 0;

//     if let JsonValue::Array(spent_utxos) = &notes["spent_utxos"] {
//         for utxo in spent_utxos {
//             if utxo["spent"] == txid || utxo["pending_spent"] == txid {
//                 transparent_notes += 1;
//             }
//         }
//     }
//     if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
//         for utxo in pending_utxos {
//             if utxo["spent"] == txid || utxo["pending_spent"] == txid {
//                 transparent_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(spent_sapling_notes) = &notes["spent_sapling_notes"] {
//         for note in spent_sapling_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 sapling_notes += 1;
//             }
//         }
//     }
//     if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
//         for note in pending_sapling_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 sapling_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(spent_orchard_notes) = &notes["spent_orchard_notes"] {
//         for note in spent_orchard_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 orchard_notes += 1;
//             }
//         }
//     }
//     if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
//         for note in pending_orchard_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 orchard_notes += 1;
//             }
//         }
//     }

//     TxNotesCount {
//         transparent_tx_notes: transparent_notes,
//         sapling_tx_notes: sapling_notes,
//         orchard_tx_notes: orchard_notes,
//     }
// }

// /// Returns number of notes created in txid as TxNotesCount (transparent_notes, sapling_notes, orchard_notes).
// pub async fn tx_outputs(client: &LightClient, txid: &str) -> TxNotesCount {
//     let notes = client.do_list_notes(true).await;

//     let mut transparent_notes = 0;
//     let mut sapling_notes = 0;
//     let mut orchard_notes = 0;

//     if let JsonValue::Array(unspent_utxos) = &notes["utxos"] {
//         for utxo in unspent_utxos {
//             if utxo["created_in_txid"] == txid {
//                 transparent_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
//         for utxo in pending_utxos {
//             if utxo["created_in_txid"] == txid {
//                 transparent_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(unspent_sapling_notes) = &notes["unspent_sapling_notes"] {
//         for note in unspent_sapling_notes {
//             if note["created_in_txid"] == txid {
//                 sapling_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
//         for note in pending_sapling_notes {
//             if note["created_in_txid"] == txid {
//                 sapling_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(unspent_orchard_notes) = &notes["unspent_orchard_notes"] {
//         for note in unspent_orchard_notes {
//             if note["created_in_txid"] == txid {
//                 orchard_notes += 1;
//             }
//         }
//     }

//     if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
//         for note in pending_orchard_notes {
//             if note["created_in_txid"] == txid {
//                 orchard_notes += 1;
//             }
//         }
//     }

//     TxNotesCount {
//         transparent_tx_notes: transparent_notes,
//         sapling_tx_notes: sapling_notes,
//         orchard_tx_notes: orchard_notes,
//     }
// }

// /// Returns total actions for txid as TxActionsCount.
// pub async fn tx_actions(
//     sender: &LightClient,
//     recipient: Option<&LightClient>,
//     txid: &str,
// ) -> TxActionsCount {
//     let tx_ins = tx_inputs(sender, txid).await;
//     let tx_outs = if let Some(rec) = recipient {
//         tx_outputs(rec, txid).await
//     } else {
//         TxNotesCount {
//             transparent_tx_notes: 0,
//             sapling_tx_notes: 0,
//             orchard_tx_notes: 0,
//         }
//     };
//     let tx_change = tx_outputs(sender, txid).await;

//     let calculated_sapling_tx_actions = cmp::max(
//         tx_ins.sapling_tx_notes,
//         tx_outs.sapling_tx_notes + tx_change.sapling_tx_notes,
//     );
//     let final_sapling_tx_actions = if calculated_sapling_tx_actions == 1 {
//         2
//     } else {
//         calculated_sapling_tx_actions
//     };

//     let calculated_orchard_tx_actions = cmp::max(
//         tx_ins.orchard_tx_notes,
//         tx_outs.orchard_tx_notes + tx_change.orchard_tx_notes,
//     );
//     let final_orchard_tx_actions = if calculated_orchard_tx_actions == 1 {
//         2
//     } else {
//         calculated_orchard_tx_actions
//     };

//     TxActionsCount {
//         transparent_tx_actions: cmp::max(
//             tx_ins.transparent_tx_notes,
//             tx_outs.transparent_tx_notes + tx_change.transparent_tx_notes,
//         ),
//         sapling_tx_actions: final_sapling_tx_actions,
//         orchard_tx_actions: final_orchard_tx_actions,
//     }
// }

// /// Returns the total transfer value of txid.
// pub async fn total_tx_value(client: &LightClient, txid: &str) -> u64 {
//     let notes = client.do_list_notes(true).await;

//     let mut tx_spend: u64 = 0;
//     let mut tx_change: u64 = 0;
//     if let JsonValue::Array(spent_utxos) = &notes["spent_utxos"] {
//         for utxo in spent_utxos {
//             if utxo["spent"] == txid || utxo["pending_spent"] == txid {
//                 tx_spend += utxo["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(pending_utxos) = &notes["pending_utxos"] {
//         for utxo in pending_utxos {
//             if utxo["spent"] == txid || utxo["pending_spent"] == txid {
//                 tx_spend += utxo["value"].as_u64().unwrap();
//             } else if utxo["created_in_txid"] == txid {
//                 tx_change += utxo["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(unspent_utxos) = &notes["utxos"] {
//         for utxo in unspent_utxos {
//             if utxo["created_in_txid"] == txid {
//                 tx_change += utxo["value"].as_u64().unwrap();
//             }
//         }
//     }

//     if let JsonValue::Array(spent_sapling_notes) = &notes["spent_sapling_notes"] {
//         for note in spent_sapling_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 tx_spend += note["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(pending_sapling_notes) = &notes["pending_sapling_notes"] {
//         for note in pending_sapling_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 tx_spend += note["value"].as_u64().unwrap();
//             } else if note["created_in_txid"] == txid {
//                 tx_change += note["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(unspent_sapling_notes) = &notes["unspent_sapling_notes"] {
//         for note in unspent_sapling_notes {
//             if note["created_in_txid"] == txid {
//                 tx_change += note["value"].as_u64().unwrap();
//             }
//         }
//     }

//     if let JsonValue::Array(spent_orchard_notes) = &notes["spent_orchard_notes"] {
//         for note in spent_orchard_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 tx_spend += note["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(pending_orchard_notes) = &notes["pending_orchard_notes"] {
//         for note in pending_orchard_notes {
//             if note["spent"] == txid || note["pending_spent"] == txid {
//                 tx_spend += note["value"].as_u64().unwrap();
//             } else if note["created_in_txid"] == txid {
//                 tx_change += note["value"].as_u64().unwrap();
//             }
//         }
//     }
//     if let JsonValue::Array(unspent_orchard_notes) = &notes["unspent_orchard_notes"] {
//         for note in unspent_orchard_notes {
//             if note["created_in_txid"] == txid {
//                 tx_change += note["value"].as_u64().unwrap();
//             }
//         }
//     }

//     tx_spend - tx_change
// }

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

/// helperized test print.
/// if someone figures out how to improve this code it can be done in one place right here.
pub(crate) fn timestamped_test_log(text: &str) {
    println!("{}: {}", crate::wallet::now(), text);
}
