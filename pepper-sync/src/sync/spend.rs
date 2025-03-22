//! Module for reading and updating wallet data related to spending

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tokio::sync::mpsc;
use zcash_client_backend::ShieldedProtocol;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{self, BlockHeight},
    transaction::TxId,
};
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest},
    error::SyncError,
    scan::{transactions::scan_transactions, DecryptedNoteData},
    wallet::{
        traits::{SyncBlocks, SyncNullifiers, SyncOutPoints, SyncTransactions},
        Locator, NullifierMap, OutputId, WalletBlock, WalletTransaction,
    },
};

use super::state;

/// Helper function for handling spend detection and the spend status of notes.
///
/// Detects if any derived nullifiers of notes in the wallet's transactions match a nullifier in the wallet's nullifier map.
/// If a spend is detected, the nullifier is removed from the nullifier map and added to the map of spend locators.
/// The spend locators are used to set the surrounding shard block ranges to be prioritised for scanning and then to
/// fetch and scan the transactions with detected spends in the case that they evaded trial decryption.
/// Finally, all notes that were detected as spent are updated with the located spending transaction.
pub(super) async fn update_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
) -> Result<(), SyncError<W::Error>>
where
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let (sapling_derived_nullifiers, orchard_derived_nullifiers) = collect_derived_nullifiers(
        wallet
            .get_wallet_transactions()
            .map_err(SyncError::WalletError)?,
    );

    let (sapling_spend_locators, orchard_spend_locators) = detect_shielded_spends(
        wallet
            .get_nullifiers_mut()
            .map_err(SyncError::WalletError)?,
        sapling_derived_nullifiers,
        orchard_derived_nullifiers,
    );

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedProtocol::Sapling,
        sapling_spend_locators.values().cloned(),
    );
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedProtocol::Orchard,
        orchard_spend_locators.values().cloned(),
    );

    // in the edge case where a spending transaction received no change, scan the transactions that evaded trial decryption
    scan_spending_transactions(
        fetch_request_sender,
        consensus_parameters,
        wallet,
        ufvks,
        sapling_spend_locators
            .values()
            .chain(orchard_spend_locators.values())
            .cloned(),
        scanned_blocks,
    )
    .await?;

    update_spent_notes(
        wallet
            .get_wallet_transactions_mut()
            .map_err(SyncError::WalletError)?,
        sapling_spend_locators,
        orchard_spend_locators,
    );

    Ok(())
}

/// For each locator, fetch the spending transaction and then scan and append to the wallet transactions.
///
/// This is only intended to be used for transactions that do not contain any incoming notes and therefore evaded
/// trial decryption.
/// For targetted scanning of transactions, locators should be added to the wallet using [`crate::add_scan_targets`] and
/// the `FoundNote` priorities will be automatically set for scan prioritisation. Transactions with incoming notes
/// are required to be scanned in the context of a scan task to correctly derive the nullifiers and positions for
/// spending.
async fn scan_spending_transactions<L, P, W>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    wallet: &mut W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    locators: L,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
) -> Result<(), SyncError<W::Error>>
where
    L: Iterator<Item = Locator>,
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let wallet_transactions = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?;
    let wallet_txids = wallet_transactions.keys().copied().collect::<HashSet<_>>();
    let mut spending_locators = BTreeSet::new();
    let mut wallet_blocks = BTreeMap::new();
    for locator in locators {
        let block_height = locator.0;
        let txid = locator.1;

        // skip if transaction already exists in the wallet
        if wallet_txids.contains(&txid) {
            continue;
        }

        spending_locators.insert(locator);

        let wallet_block = match wallet.get_wallet_block(block_height) {
            Ok(block) => block,
            Err(_) => match scanned_blocks.get(&block_height) {
                Some(block) => block.clone(),
                None => {
                    WalletBlock::from_compact_block(
                        consensus_parameters,
                        fetch_request_sender.clone(),
                        &client::get_compact_block(fetch_request_sender.clone(), block_height)
                            .await?,
                    )
                    .await?
                }
            },
        };

        wallet_blocks.insert(block_height, wallet_block);
    }

    let mut outpoint_map = BTreeMap::new(); // dummy outpoint map
    let spending_transactions = scan_transactions(
        fetch_request_sender,
        consensus_parameters,
        ufvks,
        spending_locators,
        DecryptedNoteData::new(),
        &wallet_blocks,
        &mut outpoint_map,
        HashMap::new(), // no need to scan transparent bundles as all relevant txs will not be evaded during scanning
    )
    .await?;

    wallet
        .extend_wallet_transactions(spending_transactions)
        .map_err(SyncError::WalletError)
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
        .flat_map(|note| note.nullifier)
        .collect::<Vec<_>>();
    let orchard_nullifiers = wallet_transactions
        .values()
        .flat_map(|tx| tx.orchard_notes())
        .flat_map(|note| note.nullifier)
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
        .flat_map(|nf| nullifier_map.sapling.remove_entry(nf))
        .collect();
    let orchard_spend_locators = orchard_derived_nullifiers
        .iter()
        .flat_map(|nf| nullifier_map.orchard.remove_entry(nf))
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
                .nullifier
                .and_then(|nf| sapling_spend_locators.get(&nf))
            {
                note.spending_transaction = Some(*txid);
            }
        });
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.orchard_notes_mut())
        .for_each(|note| {
            if let Some((_, txid)) = note
                .nullifier
                .and_then(|nf| orchard_spend_locators.get(&nf))
            {
                note.spending_transaction = Some(*txid);
            }
        });
}

/// Helper function for handling spend detection and the spend status of coins.
///
/// Locates any output ids of coins in the wallet's transactions which match an output id in the wallet's outpoint map.
/// If a spend is detected, the output id is removed from the outpoint map and added to the map of spend locators.
/// Finally, all coins that were detected as spent are updated with the located spending transaction.
pub(super) fn update_transparent_spends<W>(wallet: &mut W) -> Result<(), W::Error>
where
    W: SyncBlocks + SyncTransactions + SyncOutPoints,
{
    let transparent_output_ids = collect_transparent_output_ids(wallet.get_wallet_transactions()?);

    let transparent_spend_locators =
        detect_transparent_spends(wallet.get_outpoints_mut()?, transparent_output_ids);

    update_spent_coins(
        wallet.get_wallet_transactions_mut()?,
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
        .map(|coin| coin.output_id)
        .collect()
}

/// Check if any wallet coin's output id match an outpoint in the `outpoint_map`.
pub(super) fn detect_transparent_spends(
    outpoint_map: &mut BTreeMap<OutputId, Locator>,
    transparent_output_ids: Vec<OutputId>,
) -> BTreeMap<OutputId, Locator> {
    transparent_output_ids
        .iter()
        .flat_map(|output_id| outpoint_map.remove_entry(output_id))
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
            if let Some((_, txid)) = transparent_spend_locators.get(&coin.output_id) {
                coin.spending_transaction = Some(*txid);
            }
        });
}
