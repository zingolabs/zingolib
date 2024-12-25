use std::collections::{BTreeMap, HashMap, HashSet};

use incrementalmerkletree::Position;
use orchard::{
    keys::Scope,
    note_encryption::OrchardDomain,
    primitives::redpallas::{Signature, SpendAuth},
    Action,
};
use sapling_crypto::{
    bundle::{GrothProofBytes, OutputDescription},
    note_encryption::SaplingDomain,
};
use tokio::sync::mpsc;

use zcash_keys::{
    address::UnifiedAddress, encoding::encode_payment_address, keys::UnifiedFullViewingKey,
};
use zcash_note_encryption::{BatchDomain, Domain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    consensus::{self, BlockHeight, NetworkConstants},
    memo::Memo,
    transaction::{Transaction, TxId},
    zip32::AccountId,
};
use zingo_memo::ParsedMemo;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::{
    client::{self, FetchRequest},
    keys::{self, transparent::TransparentAddressId, KeyId},
    primitives::{
        Locator, NullifierMap, OrchardNote, OutPointMap, OutgoingNote, OutgoingOrchardNote,
        OutgoingSaplingNote, OutputId, SaplingNote, SyncOutgoingNotes, TransparentCoin,
        WalletBlock, WalletNote, WalletTransaction,
    },
    traits::{SyncBlocks, SyncNullifiers, SyncTransactions},
    utils,
};

use super::DecryptedNoteData;

trait ShieldedOutputExt<D: Domain>: ShieldedOutput<D, ENC_CIPHERTEXT_SIZE> {
    fn out_ciphertext(&self) -> [u8; 80];

    fn value_commitment(&self) -> D::ValueCommitment;
}

impl<A> ShieldedOutputExt<OrchardDomain> for Action<A> {
    fn out_ciphertext(&self) -> [u8; 80] {
        self.encrypted_note().out_ciphertext
    }

    fn value_commitment(&self) -> <OrchardDomain as Domain>::ValueCommitment {
        self.cv_net().clone()
    }
}

impl<Proof> ShieldedOutputExt<SaplingDomain> for OutputDescription<Proof> {
    fn out_ciphertext(&self) -> [u8; 80] {
        *self.out_ciphertext()
    }

    fn value_commitment(&self) -> <SaplingDomain as Domain>::ValueCommitment {
        self.cv().clone()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_transactions<P: consensus::Parameters>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    relevant_txids: HashSet<TxId>,
    decrypted_note_data: DecryptedNoteData,
    wallet_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    outpoint_map: &mut OutPointMap,
    transparent_addresses: HashMap<String, TransparentAddressId>,
) -> Result<HashMap<TxId, WalletTransaction>, ()> {
    let mut wallet_transactions = HashMap::with_capacity(relevant_txids.len());

    for txid in relevant_txids {
        let (transaction, block_height) =
            client::get_transaction_and_block_height(fetch_request_sender.clone(), txid)
                .await
                .unwrap();

        if transaction.txid() != txid {
            panic!("transaction txid does not match txid requested!")
        }

        // wallet block must exist, otherwise the transaction will not have access to essential data such as the time it was mined
        if let Some(wallet_block) = wallet_blocks.get(&block_height) {
            if !wallet_block.txids().contains(&transaction.txid()) {
                panic!("txid is not found in the wallet block at the transaction height!");
            }
        } else {
            panic!("wallet block at transaction height not found!");
        }

        let confirmation_status = ConfirmationStatus::Confirmed(block_height);
        let wallet_transaction = scan_transaction(
            consensus_parameters,
            ufvks,
            transaction,
            confirmation_status,
            &decrypted_note_data,
            &mut NullifierMap::new(),
            outpoint_map,
            &transparent_addresses,
        )
        .unwrap();
        wallet_transactions.insert(txid, wallet_transaction);
    }

    Ok(wallet_transactions)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_transaction<P: consensus::Parameters>(
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transaction: Transaction,
    confirmation_status: ConfirmationStatus,
    decrypted_note_data: &DecryptedNoteData,
    nullifier_map: &mut NullifierMap,
    outpoint_map: &mut OutPointMap,
    transparent_addresses: &HashMap<String, TransparentAddressId>,
) -> Result<WalletTransaction, ()> {
    // TODO: consider splitting into separate fns for pending and confirmed etc.
    // TODO: price? save in wallet block as its relative to time mined?
    let block_height = confirmation_status.get_height();
    let zip212_enforcement = zcash_primitives::transaction::components::sapling::zip212_enforcement(
        consensus_parameters,
        block_height,
    );
    let mut transparent_coins: Vec<TransparentCoin> = Vec::new();
    let mut sapling_notes: Vec<SaplingNote> = Vec::new();
    let mut orchard_notes: Vec<OrchardNote> = Vec::new();
    let mut outgoing_sapling_notes: Vec<OutgoingSaplingNote> = Vec::new();
    let mut outgoing_orchard_notes: Vec<OutgoingOrchardNote> = Vec::new();
    let mut encoded_memos = Vec::new();

    let mut sapling_ivks = Vec::new();
    let mut sapling_ovks = Vec::new();
    let mut orchard_ivks = Vec::new();
    let mut orchard_ovks = Vec::new();
    for (account_id, ufvk) in ufvks {
        if let Some(dfvk) = ufvk.sapling() {
            for scope in [Scope::External, Scope::Internal] {
                let key_id = KeyId::from_parts(*account_id, scope);
                sapling_ivks.push((
                    key_id,
                    sapling_crypto::note_encryption::PreparedIncomingViewingKey::new(
                        &dfvk.to_ivk(scope),
                    ),
                ));
                sapling_ovks.push((key_id, dfvk.to_ovk(scope)));
            }
        }

        if let Some(fvk) = ufvk.orchard() {
            for scope in [Scope::External, Scope::Internal] {
                let key_id = KeyId::from_parts(*account_id, scope);
                orchard_ivks.push((
                    key_id,
                    orchard::keys::PreparedIncomingViewingKey::new(&fvk.to_ivk(scope)),
                ));
                orchard_ovks.push((key_id, fvk.to_ovk(scope)));
            }
        }
    }

    if let Some(bundle) = transaction.transparent_bundle() {
        let transparent_outputs = &bundle.vout;
        scan_incoming_coins(
            consensus_parameters,
            &mut transparent_coins,
            transaction.txid(),
            transparent_addresses,
            transparent_outputs,
        );

        collect_outpoints(outpoint_map, transaction.txid(), block_height, bundle);
    }

    if let Some(bundle) = transaction.sapling_bundle() {
        let sapling_outputs: Vec<(SaplingDomain, OutputDescription<GrothProofBytes>)> = bundle
            .shielded_outputs()
            .iter()
            .map(|output| (SaplingDomain::new(zip212_enforcement), output.clone()))
            .collect();

        scan_incoming_notes::<
            SaplingDomain,
            OutputDescription<GrothProofBytes>,
            sapling_crypto::Note,
            sapling_crypto::Nullifier,
        >(
            &mut sapling_notes,
            transaction.txid(),
            sapling_ivks,
            &sapling_outputs,
            &decrypted_note_data.sapling_nullifiers_and_positions,
        )
        .unwrap();

        scan_outgoing_notes(
            &mut outgoing_sapling_notes,
            transaction.txid(),
            sapling_ovks,
            &sapling_outputs,
        )
        .unwrap();

        encoded_memos.append(&mut parse_encoded_memos(&sapling_notes).unwrap());
    }

    if let Some(bundle) = transaction.orchard_bundle() {
        let orchard_actions: Vec<(OrchardDomain, Action<Signature<SpendAuth>>)> = bundle
            .actions()
            .iter()
            .map(|action| (OrchardDomain::for_action(action), action.clone()))
            .collect();

        scan_incoming_notes::<
            OrchardDomain,
            Action<Signature<SpendAuth>>,
            orchard::Note,
            orchard::note::Nullifier,
        >(
            &mut orchard_notes,
            transaction.txid(),
            orchard_ivks,
            &orchard_actions,
            &decrypted_note_data.orchard_nullifiers_and_positions,
        )
        .unwrap();

        scan_outgoing_notes(
            &mut outgoing_orchard_notes,
            transaction.txid(),
            orchard_ovks,
            &orchard_actions,
        )
        .unwrap();

        encoded_memos.append(&mut parse_encoded_memos(&orchard_notes).unwrap());
    }

    // collect nullifiers for pending transactions
    // nullifiers for confirmed transactions are collected during compact block scanning
    if !confirmation_status.is_confirmed() {
        collect_nullifiers(nullifier_map, block_height, &transaction);
    }

    for encoded_memo in encoded_memos {
        match encoded_memo {
            ParsedMemo::Version0 { uas } => {
                add_recipient_unified_address(
                    consensus_parameters,
                    uas.clone(),
                    &mut outgoing_sapling_notes,
                );
                add_recipient_unified_address(
                    consensus_parameters,
                    uas,
                    &mut outgoing_orchard_notes,
                );
            }
            ParsedMemo::Version1 {
                uas,
                rejection_address_indexes: _,
            } => {
                add_recipient_unified_address(
                    consensus_parameters,
                    uas.clone(),
                    &mut outgoing_sapling_notes,
                );
                add_recipient_unified_address(
                    consensus_parameters,
                    uas,
                    &mut outgoing_orchard_notes,
                );

                // TODO: handle rejection addresses from encoded memos
            }
            _ => panic!(
                "memo version not supported. please ensure that your software is up-to-date."
            ),
        }
    }

    // TODO: consider adding nullifiers and transparent outpoint data for efficiency

    Ok(WalletTransaction::from_parts(
        transaction.txid(),
        transaction,
        confirmation_status,
        sapling_notes,
        orchard_notes,
        outgoing_sapling_notes,
        outgoing_orchard_notes,
        transparent_coins,
    ))
}

fn scan_incoming_coins<P: consensus::Parameters>(
    consensus_parameters: &P,
    transparent_coins: &mut Vec<TransparentCoin>,
    txid: TxId,
    transparent_addresses: &HashMap<String, TransparentAddressId>,
    transparent_outputs: &[zcash_primitives::transaction::components::TxOut],
) {
    for (output_index, output) in transparent_outputs.iter().enumerate() {
        if let Some(address) = output.recipient_address() {
            let encoded_address = keys::transparent::encode_address(consensus_parameters, address);
            if let Some((address, key_id)) = transparent_addresses.get_key_value(&encoded_address) {
                let output_id = OutputId::from_parts(txid, output_index);

                transparent_coins.push(TransparentCoin::from_parts(
                    output_id,
                    *key_id,
                    address.clone(),
                    output.script_pubkey.clone(),
                    output.value,
                    None,
                ));
            }
        }
    }
}

fn scan_incoming_notes<D, Op, N, Nf>(
    wallet_notes: &mut Vec<WalletNote<N, Nf>>,
    txid: TxId,
    ivks: Vec<(KeyId, D::IncomingViewingKey)>,
    outputs: &[(D, Op)],
    nullifiers_and_positions: &HashMap<OutputId, (Nf, Position)>,
) -> Result<(), ()>
where
    D: BatchDomain<Note = N>,
    D::Memo: AsRef<[u8]>,
    Op: ShieldedOutput<D, ENC_CIPHERTEXT_SIZE>,
    Nf: Copy,
{
    let (key_ids, ivks): (Vec<_>, Vec<_>) = ivks.into_iter().unzip();

    for (output_index, output) in zcash_note_encryption::batch::try_note_decryption(&ivks, outputs)
        .into_iter()
        .enumerate()
    {
        if let Some(((note, _, memo_bytes), key_index)) = output {
            let output_id = OutputId::from_parts(txid, output_index);
            let (nullifier, position) = nullifiers_and_positions.get(&output_id).unwrap();
            wallet_notes.push(WalletNote::from_parts(
                output_id,
                key_ids[key_index],
                note,
                Some(*nullifier),
                Some(*position),
                Memo::from_bytes(memo_bytes.as_ref()).unwrap(),
                None,
            ));
        }
    }

    Ok(())
}

fn scan_outgoing_notes<D, Op, N>(
    outgoing_notes: &mut Vec<OutgoingNote<N>>,
    txid: TxId,
    ovks: Vec<(KeyId, D::OutgoingViewingKey)>,
    outputs: &[(D, Op)],
) -> Result<(), ()>
where
    D: Domain<Note = N>,
    D::Memo: AsRef<[u8]>,
    Op: ShieldedOutputExt<D>,
{
    let (key_ids, ovks): (Vec<_>, Vec<_>) = ovks.into_iter().unzip();

    for (output_index, (domain, output)) in outputs.iter().enumerate() {
        if let Some(((note, _, memo_bytes), key_index)) = try_output_recovery_with_ovks(
            domain,
            &ovks,
            output,
            &output.value_commitment(),
            &output.out_ciphertext(),
        ) {
            outgoing_notes.push(OutgoingNote::from_parts(
                OutputId::from_parts(txid, output_index),
                key_ids[key_index],
                note,
                Memo::from_bytes(memo_bytes.as_ref()).unwrap(),
                None,
            ));
        }
    }

    Ok(())
}

#[allow(clippy::type_complexity)]
fn try_output_recovery_with_ovks<D: Domain, Output: ShieldedOutput<D, ENC_CIPHERTEXT_SIZE>>(
    domain: &D,
    ovks: &[D::OutgoingViewingKey],
    output: &Output,
    cv: &D::ValueCommitment,
    out_ciphertext: &[u8; zcash_note_encryption::OUT_CIPHERTEXT_SIZE],
) -> Option<((D::Note, D::Recipient, D::Memo), usize)> {
    for (key_index, ovk) in ovks.iter().enumerate() {
        if let Some(decrypted_output) = zcash_note_encryption::try_output_recovery_with_ovk(
            domain,
            ovk,
            output,
            cv,
            out_ciphertext,
        ) {
            return Some((decrypted_output, key_index));
        }
    }
    None
}

fn parse_encoded_memos<N, Nf: Copy>(
    wallet_notes: &[WalletNote<N, Nf>],
) -> Result<Vec<ParsedMemo>, ()> {
    let encoded_memos = wallet_notes
        .iter()
        .flat_map(|note| {
            if let Memo::Arbitrary(ref encoded_memo_bytes) = note.memo() {
                Some(zingo_memo::parse_zingo_memo(*encoded_memo_bytes.as_ref()).unwrap())
            } else {
                None
            }
        })
        .collect();

    Ok(encoded_memos)
}

// TODO: consider comparing types instead of encoding to string
fn add_recipient_unified_address<P, Nz>(
    parameters: &P,
    unified_addresses: Vec<UnifiedAddress>,
    outgoing_notes: &mut [OutgoingNote<Nz>],
) where
    P: consensus::Parameters + NetworkConstants,
    OutgoingNote<Nz>: SyncOutgoingNotes,
{
    for ua in unified_addresses {
        let ua_receivers = [
            utils::encode_orchard_receiver(parameters, ua.orchard().unwrap()).unwrap(),
            encode_payment_address(
                parameters.hrp_sapling_payment_address(),
                ua.sapling().unwrap(),
            ),
            utils::address_from_pubkeyhash(parameters, ua.transparent().unwrap()),
            ua.encode(parameters),
        ];
        outgoing_notes
            .iter_mut()
            .filter(|note| ua_receivers.contains(&note.encoded_recipient(parameters)))
            .for_each(|note| {
                note.set_recipient_ua(Some(ua.clone()));
            });
    }
}

/// Converts and adds the nullifiers from a transaction to the nullifier map
fn collect_nullifiers(
    nullifier_map: &mut NullifierMap,
    block_height: BlockHeight,
    transaction: &Transaction,
) {
    if let Some(bundle) = transaction.sapling_bundle() {
        bundle
            .shielded_spends()
            .iter()
            .map(|spend| spend.nullifier())
            .for_each(|nullifier| {
                nullifier_map
                    .sapling_mut()
                    .insert(*nullifier, (block_height, transaction.txid()));
            });
    }
    if let Some(bundle) = transaction.orchard_bundle() {
        bundle
            .actions()
            .iter()
            .map(|action| action.nullifier())
            .for_each(|nullifier| {
                nullifier_map
                    .orchard_mut()
                    .insert(*nullifier, (block_height, transaction.txid()));
            });
    }
}

/// Adds the outpoints from a transparent bundle to the outpoint map.
fn collect_outpoints<A: zcash_primitives::transaction::components::transparent::Authorization>(
    outpoint_map: &mut OutPointMap,
    txid: TxId,
    block_height: BlockHeight,
    transparent_bundle: &zcash_primitives::transaction::components::transparent::Bundle<A>,
) {
    transparent_bundle
        .vin
        .iter()
        .map(|txin| &txin.prevout)
        .for_each(|outpoint| {
            outpoint_map.inner_mut().insert(
                OutputId::from_parts(*outpoint.txid(), outpoint.n() as usize),
                (block_height, txid),
            );
        });
}

/// For each `locator`, fetch the transaction and then scan and append to the wallet transactions.
///
/// This is not intended to be used outside of the context of processing scan results of a scanned range.
/// This function will panic if the wallet block for a given locator does not exist in the wallet.
pub(crate) async fn scan_located_transactions<L, P, W>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    wallet: &mut W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    locators: L,
) -> Result<(), ()>
where
    L: Iterator<Item = Locator>,
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let wallet_transactions = wallet.get_wallet_transactions().unwrap();
    let wallet_txids = wallet_transactions.keys().copied().collect::<HashSet<_>>();
    let mut spending_txids = HashSet::new();
    let mut wallet_blocks = BTreeMap::new();
    for (block_height, txid) in locators {
        // skip if transaction already exists in the wallet
        if wallet_txids.contains(&txid) {
            continue;
        }

        spending_txids.insert(txid);
        wallet_blocks.insert(
            block_height,
            wallet.get_wallet_block(block_height).expect(
                "wallet block should be in the wallet due to the context of this functions purpose",
            ),
        );
    }

    let mut outpoint_map = OutPointMap::new(); // dummy outpoint map
    let spending_transactions = scan_transactions(
        fetch_request_sender,
        consensus_parameters,
        ufvks,
        spending_txids,
        DecryptedNoteData::new(),
        &wallet_blocks,
        &mut outpoint_map,
        HashMap::new(), // no need to scan transparent bundles as all relevant txs will not be evaded during scanning
    )
    .await
    .unwrap();
    wallet
        .extend_wallet_transactions(spending_transactions)
        .unwrap();

    Ok(())
}
