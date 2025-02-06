//! Module for reading and updating wallet data related to spending

use std::collections::{BTreeMap, HashMap};

use tokio::sync::mpsc;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{self, BlockHeight},
    transaction::TxId,
};
use zip32::AccountId;

use crate::{
    client::FetchRequest,
    primitives::{Locator, NullifierMap, OutPointMap, OutputId, WalletTransaction},
    scan::transactions::scan_located_transactions,
    traits::{SyncBlocks, SyncNullifiers, SyncOutPoints, SyncTransactions},
};

/// Helper function for handling spend detection and the spend status of notes.
///
/// Locates any nullifiers of notes in the wallet's transactions which match a nullifier in the wallet's nullifier map.
/// If a spend is detected, the nullifier is removed from the nullifier map and added to the map of spend locators.
/// The spend locators are then used to fetch and scan the transactions with detected spends.
/// Finally, all notes that were detected as spent are updated with the located spending transaction.
pub(super) async fn update_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
) -> Result<(), ()>
where
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let (sapling_derived_nullifiers, orchard_derived_nullifiers) =
        collect_derived_nullifiers(wallet.get_wallet_transactions().unwrap());

    let (sapling_spend_locators, orchard_spend_locators) = detect_shielded_spends(
        wallet.get_nullifiers_mut().unwrap(),
        sapling_derived_nullifiers,
        orchard_derived_nullifiers,
    );

    // in the edge case where a spending transaction received no change, scan the transactions that evaded trial decryption
    scan_located_transactions(
        fetch_request_sender,
        consensus_parameters,
        wallet,
        ufvks,
        sapling_spend_locators
            .values()
            .chain(orchard_spend_locators.values())
            .cloned(),
    )
    .await
    .unwrap();

    update_spent_notes(
        wallet.get_wallet_transactions_mut().unwrap(),
        sapling_spend_locators,
        orchard_spend_locators,
    );

    Ok(())
}

/// Collects the derived nullifiers from each note in the wallet
pub(super) fn collect_derived_nullifiers(
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> (
    Vec<sapling_crypto::Nullifier>,
    Vec<orchard::note::Nullifier>,
) {
    let sapling_nullifiers = wallet_transactions
        .values()
        .flat_map(|tx| tx.sapling_notes())
        .flat_map(|note| note.nullifier())
        .collect::<Vec<_>>();
    let orchard_nullifiers = wallet_transactions
        .values()
        .flat_map(|tx| tx.orchard_notes())
        .flat_map(|note| note.nullifier())
        .collect::<Vec<_>>();

    (sapling_nullifiers, orchard_nullifiers)
}

/// Check if any wallet note's derived nullifiers match a nullifier in the `nullifier_map`.
pub(super) fn detect_shielded_spends(
    nullifier_map: &mut NullifierMap,
    sapling_derived_nullifiers: Vec<sapling_crypto::Nullifier>,
    orchard_derived_nullifiers: Vec<orchard::note::Nullifier>,
) -> (
    BTreeMap<sapling_crypto::Nullifier, Locator>,
    BTreeMap<orchard::note::Nullifier, Locator>,
) {
    let sapling_spend_locators = sapling_derived_nullifiers
        .iter()
        .flat_map(|nf| nullifier_map.sapling_mut().remove_entry(nf))
        .collect();
    let orchard_spend_locators = orchard_derived_nullifiers
        .iter()
        .flat_map(|nf| nullifier_map.orchard_mut().remove_entry(nf))
        .collect();

    (sapling_spend_locators, orchard_spend_locators)
}

/// Update the spending transaction for all notes where the derived nullifier matches the nullifier in the spend locator map.
/// The items in the spend locator map are taken directly from the nullifier map during spend detection.
pub(super) fn update_spent_notes(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    sapling_spend_locators: BTreeMap<sapling_crypto::Nullifier, Locator>,
    orchard_spend_locators: BTreeMap<orchard::note::Nullifier, Locator>,
) {
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.sapling_notes_mut())
        .for_each(|note| {
            if let Some((_, txid)) = note
                .nullifier()
                .and_then(|nf| sapling_spend_locators.get(&nf))
            {
                note.set_spending_transaction(Some(*txid));
            }
        });
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.orchard_notes_mut())
        .for_each(|note| {
            if let Some((_, txid)) = note
                .nullifier()
                .and_then(|nf| orchard_spend_locators.get(&nf))
            {
                note.set_spending_transaction(Some(*txid));
            }
        });
}

/// Helper function for handling spend detection and the spend status of coins.
///
/// Locates any output ids of coins in the wallet's transactions which match an output id in the wallet's outpoint map.
/// If a spend is detected, the output id is removed from the outpoint map and added to the map of spend locators.
/// Finally, all coins that were detected as spent are updated with the located spending transaction.
pub(super) fn update_transparent_spends<W>(wallet: &mut W) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncOutPoints,
{
    let transparent_output_ids =
        collect_transparent_output_ids(wallet.get_wallet_transactions().unwrap());

    let transparent_spend_locators =
        detect_transparent_spends(wallet.get_outpoints_mut().unwrap(), transparent_output_ids);

    update_spent_coins(
        wallet.get_wallet_transactions_mut().unwrap(),
        transparent_spend_locators,
    );

    Ok(())
}

/// Collects the output ids from each coin in the wallet
pub(super) fn collect_transparent_output_ids(
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> Vec<OutputId> {
    wallet_transactions
        .values()
        .flat_map(|tx| tx.transparent_coins())
        .map(|coin| coin.output_id())
        .collect()
}

/// Check if any wallet coin's output id match an outpoint in the `outpoint_map`.
pub(super) fn detect_transparent_spends(
    outpoint_map: &mut OutPointMap,
    transparent_output_ids: Vec<OutputId>,
) -> BTreeMap<OutputId, Locator> {
    transparent_output_ids
        .iter()
        .flat_map(|output_id| outpoint_map.inner_mut().remove_entry(output_id))
        .collect()
}

/// Update the spending transaction for all coins where the output id matches the output id in the spend locator map.
/// The items in the spend locator map are taken directly from the outpoint map during spend detection.
pub(super) fn update_spent_coins(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    transparent_spend_locators: BTreeMap<OutputId, (BlockHeight, TxId)>,
) {
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.transparent_coins_mut())
        .for_each(|coin| {
            if let Some((_, txid)) = transparent_spend_locators.get(&coin.output_id()) {
                coin.set_spending_transaction(Some(*txid));
            }
        });
}
