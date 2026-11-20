use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
};

use tokio::sync::mpsc;

use incrementalmerkletree::Position;
use orchard::{
    Action,
    keys::Scope,
    note_encryption::{IronwoodDomain, OrchardDomain},
    primitives::redpallas::{Signature, SpendAuth},
};
use sapling_crypto::{
    bundle::{GrothProofBytes, OutputDescription},
    note_encryption::SaplingDomain,
};
use zcash_keys::{
    address::UnifiedAddress,
    keys::{OutgoingViewingKey, UnifiedFullViewingKey},
};
use zcash_note_encryption::{BatchDomain, Domain, ShieldedOutput};
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight, NetworkConstants},
    memo::Memo,
};

use zcash_transparent::bundle::{
    Authorization as TransparentAuthorization, Bundle as TransparentBundle, TxIn,
};
use zip32::AccountId;
use zl_common::memo::ParsedMemo;
use zl_common::status::ConfirmationStatus;

use crate::engine::{
    KeyIdOps, OutgoingNoteOps, ScanTargetOps, TransactionRecordOps, TransactionRecordParts,
    TransparentCoinOps, WalletNoteOps,
};
use crate::{
    client::{
        self, FetchRequest,
        cover::{CoverFetch, CoverRequest},
    },
    error::{ScanError, ServerError, status_is_network_weather},
    keys::{self, KeyId, transparent::TransparentAddressId},
    wallet::{
        IronwoodNote, NoteInterface, NullifierMap, OrchardNote, OutgoingIronwoodNote, OutgoingNote,
        OutgoingNoteInterface, OutgoingOrchardNote, OutgoingSaplingNote, OutputId, SaplingNote,
        ScanTarget, ScanTargetOrigin, TransactionRecord, TransparentCoin, WalletBlock, WalletNote,
    },
};

use super::DecryptedNoteData;

trait ShieldedOutputExt<D: Domain>: ShieldedOutput<D> {
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

impl<A> ShieldedOutputExt<IronwoodDomain> for Action<A> {
    fn out_ciphertext(&self) -> [u8; 80] {
        self.encrypted_note().out_ciphertext
    }

    fn value_commitment(&self) -> <IronwoodDomain as Domain>::ValueCommitment {
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
/// The transactions scanned for a set of scan targets, and the external targets whose transaction the server did
/// not serve.
pub(crate) struct ScannedTransactions {
    pub(crate) transactions: HashMap<TxId, TransactionRecord>,
    pub(crate) dropped_targets: BTreeSet<ScanTarget>,
}

/// Serializes the unconfirmed transactions of `wallet_transactions`, keyed by txid.
pub(crate) fn serialize_pending_transactions<'a>(
    wallet_transactions: impl Iterator<Item = &'a TransactionRecord>,
) -> HashMap<TxId, Vec<u8>> {
    wallet_transactions
        .filter(|wallet_transaction| !wallet_transaction.status().is_confirmed())
        .filter_map(|wallet_transaction| {
            let mut transaction_bytes = Vec::new();
            wallet_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .ok()
                .map(|()| (wallet_transaction.txid(), transaction_bytes))
        })
        .collect()
}

/// Returns the pending transaction stored for `scan_target`, or `None` for an external target.
fn stored_pending_transaction(
    consensus_parameters: &impl consensus::Parameters,
    pending_transactions: &HashMap<TxId, Vec<u8>>,
    scan_target: &ScanTarget,
    txid: TxId,
) -> Option<Transaction> {
    if scan_target.origin() != ScanTargetOrigin::Internal {
        return None;
    }

    Transaction::read(
        &pending_transactions.get(&txid)?[..],
        consensus::BranchId::for_height(consensus_parameters, scan_target.block_height()),
    )
    .ok()
    .filter(|transaction| transaction.txid() == txid)
}

/// Scans the transactions of `scan_targets`.
///
/// Transactions missing from `pending_transactions` are fetched under cover, with every transaction of each
/// block of `scanned_heights` in the open group at the chain tip.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_transactions(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_targets: BTreeSet<ScanTarget>,
    decrypted_note_data: DecryptedNoteData,
    wallet_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    outpoint_map: &mut BTreeMap<OutputId, ScanTarget>,
    transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    pending_transactions: &HashMap<TxId, Vec<u8>>,
    scanned_heights: Range<BlockHeight>,
    chain_height: BlockHeight,
) -> Result<ScannedTransactions, ScanError> {
    let mut transactions = HashMap::with_capacity(scan_targets.len());
    let mut dropped_targets = BTreeSet::new();

    let mut stored_transactions = HashMap::new();
    let mut fetch_targets = Vec::new();
    let mut timed_heights = Vec::new();
    for scan_target in &scan_targets {
        let Some(txid) = scan_target.txid() else {
            continue;
        };

        match stored_pending_transaction(
            consensus_parameters,
            pending_transactions,
            scan_target,
            txid,
        ) {
            Some(transaction) => {
                stored_transactions.insert(txid, transaction);
                if !wallet_blocks.contains_key(&scan_target.block_height()) {
                    timed_heights.push(scan_target.block_height());
                }
            }
            None => fetch_targets.push((scan_target.block_height(), txid)),
        }
    }
    let cover_request = CoverRequest {
        targets: fetch_targets,
        scanned_heights,
        timed_heights,
        lowest_height: crate::wallet::PoolActivation::of(
            consensus_parameters,
            ShieldedPool::Sapling,
        )
        .map(|activation| activation.height()),
        chain_height,
    };
    let CoverFetch {
        target_replies: mut fetched_transactions,
        block_times,
    } = if cover_request.is_empty() {
        CoverFetch::default()
    } else {
        client::fetch_cover(fetch_request_sender.clone(), cover_request).await?
    };

    for scan_target in scan_targets {
        let Some(txid) = scan_target.txid() else {
            continue;
        };

        let (transaction, block_height) = if let Some(transaction) =
            stored_transactions.remove(&txid)
        {
            (transaction, scan_target.block_height())
        } else {
            let Some(fetched_transaction) = fetched_transactions.remove(&txid) else {
                continue;
            };

            match fetched_transaction
                .map_err(ServerError::RequestFailed)
                .and_then(|raw_transaction| {
                    client::parse_raw_transaction(consensus_parameters, &raw_transaction)
                }) {
                Ok(fetched) => fetched,
                // the server answered that it does not serve the transaction. a consumer's target may name a txid
                // that never existed, or one a re-org removed, and is dropped rather than failing every scan of its
                // range. a target the sync engine discovered came from the served data, so the same answer is an
                // inconsistency in that data and is reported with the recommendation to change server. network
                // weather is neither and is reported as the transient failure it is.
                Err(ServerError::RequestFailed(status)) if !status_is_network_weather(&status) => {
                    match scan_target.origin() {
                        ScanTargetOrigin::External => {
                            tracing::warn!(
                                "scan target {txid} at block {} dropped: the server does not serve the transaction. {status}",
                                scan_target.block_height()
                            );
                            dropped_targets.insert(scan_target);
                            continue;
                        }
                        ScanTargetOrigin::Internal => {
                            return Err(ScanError::TransactionNotServed {
                                txid,
                                block_height: scan_target.block_height(),
                                status,
                            });
                        }
                    }
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        };

        if transaction.txid() != txid {
            return Err(ScanError::IncorrectTxid {
                txid_requested: txid,
                txid_returned: transaction.txid(),
            });
        }

        let datetime = match wallet_blocks
            .get(&block_height)
            .map(|wallet_block| wallet_block.time())
            .or_else(|| block_times.get(&block_height).copied())
        {
            Some(datetime) => datetime,
            None => {
                client::get_compact_block(fetch_request_sender.clone(), block_height)
                    .await?
                    .time
            }
        };

        let confirmation_status = ConfirmationStatus::Confirmed(block_height);
        let wallet_transaction = scan_transaction(
            consensus_parameters,
            ufvks,
            txid,
            transaction,
            confirmation_status,
            Some(&decrypted_note_data),
            &mut NullifierMap::new(),
            outpoint_map,
            &transparent_inuse_addresses,
            datetime,
        )?;
        transactions.insert(txid, wallet_transaction);
    }

    Ok(ScannedTransactions {
        transactions,
        dropped_targets,
    })
}

/// Scans `transaction` with the given `status` and returns [`crate::wallet::TransactionRecord`], decrypting all
/// incoming and outgoing notes with `ufvks` and adding any transparent coins matching `transparent_addresses`.
///
/// `decrypted_note_data` will be `None` for pending transactions. For confirmed transactions, it must contain the
/// nullifiers and positions for each note to be decrypted. This will have been obtained during compact block scanning.
///
/// All inputs in `transaction` are inserted into the `nullifier_map` and `outpoint_map` to be used for spend detection.
/// For pending transactions, new maps are used instead of the wallet's maps as to keep confirmed spends isolated.
///
/// `txid` is used instead of `transaction.txid()` due to darkside testing bug.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_transaction(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    txid: TxId,
    transaction: Transaction,
    status: ConfirmationStatus,
    decrypted_note_data: Option<&DecryptedNoteData>,
    nullifier_map: &mut NullifierMap,
    outpoint_map: &mut BTreeMap<OutputId, ScanTarget>,
    transparent_inuse_addresses: &HashMap<String, TransparentAddressId>,
    datetime: u32,
) -> Result<TransactionRecord, ScanError> {
    let block_height = status.get_height();
    let zip212_enforcement = zcash_primitives::transaction::components::sapling::zip212_enforcement(
        consensus_parameters,
        block_height,
    );
    let mut transparent_coins: Vec<TransparentCoin> = Vec::new();
    let mut sapling_notes: Vec<SaplingNote> = Vec::new();
    let mut orchard_notes: Vec<OrchardNote> = Vec::new();
    let mut ironwood_notes: Vec<IronwoodNote> = Vec::new();
    let mut outgoing_sapling_notes: Vec<OutgoingSaplingNote> = Vec::new();
    let mut outgoing_orchard_notes: Vec<OutgoingOrchardNote> = Vec::new();
    let mut outgoing_ironwood_notes: Vec<OutgoingIronwoodNote> = Vec::new();
    let mut encoded_memos = Vec::new();

    let mut sapling_ivks = Vec::new();
    let mut sapling_ovks = Vec::new();
    let mut orchard_ivks = Vec::new();
    let mut orchard_ovks = Vec::new();
    let mut ironwood_ivks = Vec::new();
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
                add_unified_ovk(
                    &mut sapling_ovks,
                    &mut orchard_ovks,
                    key_id,
                    dfvk.to_ovk(scope).into(),
                );
            }
        }

        if let Some(fvk) = ufvk.orchard() {
            for scope in [Scope::External, Scope::Internal] {
                let key_id = KeyId::from_parts(*account_id, scope);
                orchard_ivks.push((
                    key_id,
                    orchard::keys::PreparedIncomingViewingKey::new(&fvk.to_ivk(scope)),
                ));
                // Ironwood outputs decrypt with the same orchard key material.
                ironwood_ivks.push((
                    key_id,
                    orchard::keys::PreparedIncomingViewingKey::new(&fvk.to_ivk(scope)),
                ));
                add_unified_ovk(
                    &mut sapling_ovks,
                    &mut orchard_ovks,
                    key_id,
                    fvk.to_ovk(scope).into(),
                );
            }
        }

        if let Some(tkeys) = ufvk.p2pkh() {
            add_unified_ovk(
                &mut sapling_ovks,
                &mut orchard_ovks,
                KeyId::from_parts(*account_id, Scope::External),
                OutgoingViewingKey::from(tkeys.external_ovk().as_bytes()),
            );
            add_unified_ovk(
                &mut sapling_ovks,
                &mut orchard_ovks,
                KeyId::from_parts(*account_id, Scope::Internal),
                OutgoingViewingKey::from(tkeys.internal_ovk().as_bytes()),
            );
        }
    }

    if let Some(bundle) = transaction.transparent_bundle() {
        let transparent_outputs = &bundle.vout;
        scan_incoming_coins(
            consensus_parameters,
            &mut transparent_coins,
            txid,
            transparent_inuse_addresses,
            transparent_outputs,
        );

        collect_outpoints(outpoint_map, txid, block_height, bundle);
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
            crate::wallet::Sapling,
        >(
            &mut sapling_notes,
            txid,
            sapling_ivks,
            &sapling_outputs,
            decrypted_note_data.map(|d| &d.sapling_nullifiers_and_positions),
        )?;

        scan_outgoing_notes(
            &mut outgoing_sapling_notes,
            txid,
            sapling_ovks,
            &sapling_outputs,
        )?;

        encoded_memos.append(&mut parse_encoded_memos(&sapling_notes));
    }

    // Ironwood outgoing notes recover with the same orchard outgoing keys.
    let ironwood_ovks = orchard_ovks.clone();

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
            crate::wallet::Orchard,
        >(
            &mut orchard_notes,
            txid,
            orchard_ivks,
            &orchard_actions,
            decrypted_note_data.map(|d| &d.orchard_nullifiers_and_positions),
        )?;

        scan_outgoing_notes(
            &mut outgoing_orchard_notes,
            txid,
            orchard_ovks,
            &orchard_actions,
        )?;

        encoded_memos.append(&mut parse_encoded_memos(&orchard_notes));
    }

    if let Some(bundle) = transaction.ironwood_bundle() {
        let ironwood_actions: Vec<(IronwoodDomain, Action<Signature<SpendAuth>>)> = bundle
            .actions()
            .iter()
            .map(|action| (IronwoodDomain::for_action(action), action.clone()))
            .collect();

        scan_incoming_notes::<
            IronwoodDomain,
            Action<Signature<SpendAuth>>,
            orchard::Note,
            orchard::note::Nullifier,
            crate::wallet::Ironwood,
        >(
            &mut ironwood_notes,
            txid,
            ironwood_ivks,
            &ironwood_actions,
            decrypted_note_data.map(|d| &d.ironwood_nullifiers_and_positions),
        )?;

        scan_outgoing_notes(
            &mut outgoing_ironwood_notes,
            txid,
            ironwood_ovks,
            &ironwood_actions,
        )?;

        encoded_memos.append(&mut parse_encoded_memos(&ironwood_notes));
    }

    // collect nullifiers for pending transactions
    // nullifiers for confirmed transactions are collected during compact block scanning
    if status.is_pending() {
        collect_nullifiers(nullifier_map, block_height, txid, &transaction);
    }

    for uas in encoded_memos
        .into_iter()
        .map(ParsedMemo::into_unified_addresses)
    {
        add_recipient_unified_address(
            consensus_parameters,
            uas.clone(),
            &mut outgoing_sapling_notes,
        )?;
        add_recipient_unified_address(
            consensus_parameters,
            uas.clone(),
            &mut outgoing_orchard_notes,
        )?;
        add_recipient_unified_address(consensus_parameters, uas, &mut outgoing_ironwood_notes)?;
    }

    Ok(TransactionRecord::from_parts(TransactionRecordParts {
        txid,
        status,
        transaction,
        datetime,
        transparent_coins,
        sapling_notes,
        orchard_notes,
        ironwood_notes,
        outgoing_sapling_notes,
        outgoing_orchard_notes,
        outgoing_ironwood_notes,
    }))
}

fn add_unified_ovk(
    sapling_ovks: &mut Vec<(KeyId, sapling_crypto::keys::OutgoingViewingKey)>,
    orchard_ovks: &mut Vec<(KeyId, orchard::keys::OutgoingViewingKey)>,
    key_id: KeyId,
    ovk: OutgoingViewingKey,
) {
    sapling_ovks.push((key_id, ovk.into()));
    orchard_ovks.push((key_id, ovk.into()));
}

fn scan_incoming_coins<P: consensus::Parameters>(
    consensus_parameters: &P,
    transparent_coins: &mut Vec<TransparentCoin>,
    txid: TxId,
    transparent_inuse_addresses: &HashMap<String, TransparentAddressId>,
    transparent_outputs: &[zcash_transparent::bundle::TxOut],
) {
    for (output_index, output) in transparent_outputs.iter().enumerate() {
        if let Some(address) = output.recipient_address() {
            let encoded_address = keys::transparent::encode_address(consensus_parameters, address);
            if let Some((address, key_id)) =
                transparent_inuse_addresses.get_key_value(&encoded_address)
            {
                let output_id = OutputId::new(
                    txid,
                    output_index
                        .try_into()
                        .expect("output indexes should be valid u32"),
                );

                transparent_coins.push(TransparentCoin::from_parts(
                    output_id,
                    *key_id,
                    address.clone(),
                    output.script_pubkey().clone(),
                    output.value(),
                ));
            }
        }
    }
}

fn scan_incoming_notes<D, Op, N, Nf, P>(
    wallet_notes: &mut Vec<WalletNote<N, Nf, P>>,
    txid: TxId,
    ivks: Vec<(KeyId, D::IncomingViewingKey)>,
    outputs: &[(D, Op)],
    nullifiers_and_positions: Option<&HashMap<OutputId, (Nf, Position)>>,
) -> Result<(), ScanError>
where
    D: BatchDomain<Note = N>,
    D::Memo: AsRef<[u8]>,
    Op: ShieldedOutput<D>,
    Nf: Copy,
{
    let (key_ids, ivks): (Vec<_>, Vec<_>) = ivks.into_iter().unzip();

    for (output_index, output) in zcash_note_encryption::batch::try_note_decryption(&ivks, outputs)
        .into_iter()
        .enumerate()
    {
        if let Some(((note, _, memo_bytes), key_index)) = output {
            let output_id = OutputId::new(
                txid,
                output_index
                    .try_into()
                    .expect("output indexes should be valid u32"),
            );
            let (nullifier, position) = nullifiers_and_positions.map_or(Ok((None, None)), |m| {
                m.get(&output_id)
                    .map(|(nf, pos)| (Some(*nf), Some(*pos)))
                    .ok_or(ScanError::DecryptedNoteDataNotFound(output_id))
            })?;
            wallet_notes.push(WalletNote::from_parts(
                output_id,
                key_ids[key_index],
                note,
                nullifier,
                position,
                Memo::from_bytes(memo_bytes.as_ref())?,
            ));
        }
    }

    Ok(())
}

fn scan_outgoing_notes<D, Op, N, P>(
    outgoing_notes: &mut Vec<OutgoingNote<N, P>>,
    txid: TxId,
    ovks: Vec<(KeyId, D::OutgoingViewingKey)>,
    outputs: &[(D, Op)],
) -> Result<(), ScanError>
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
                OutputId::new(
                    txid,
                    output_index
                        .try_into()
                        .expect("output indexes should be valid u32"),
                ),
                key_ids[key_index],
                note,
                Memo::from_bytes(memo_bytes.as_ref())?,
            ));
        }
    }

    Ok(())
}

#[allow(clippy::type_complexity)]
fn try_output_recovery_with_ovks<D: Domain, Output: ShieldedOutput<D>>(
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

fn parse_encoded_memos<N, Nf: Copy, P>(wallet_notes: &[WalletNote<N, Nf, P>]) -> Vec<ParsedMemo>
where
    WalletNote<N, Nf, P>: NoteInterface,
{
    wallet_notes
        .iter()
        .filter_map(|note| {
            if let Memo::Arbitrary(encoded_memo_bytes) = note.memo() {
                match zl_common::memo::parse_zingo_memo(*encoded_memo_bytes.as_ref()) {
                    Ok(encoded_memo) => Some(encoded_memo),
                    Err(e) => {
                        tracing::error!("Failed to decode memo data. {e}");
                        None
                    }
                }
            } else {
                None
            }
        })
        .collect()
}

fn add_recipient_unified_address<P, Nz, Pool>(
    consensus_parameters: &P,
    unified_addresses: Vec<UnifiedAddress>,
    outgoing_notes: &mut [OutgoingNote<Nz, Pool>],
) -> Result<(), ScanError>
where
    P: consensus::Parameters + NetworkConstants,
    OutgoingNote<Nz, Pool>: OutgoingNoteInterface,
{
    for unified_address in unified_addresses {
        let encoded_address = match <OutgoingNote<Nz, Pool>>::SHIELDED_PROTOCOL {
            ShieldedPool::Sapling => unified_address.sapling().map(|address| {
                Ok(zcash_keys::encoding::encode_payment_address(
                    zcash_address::constants::hrp_sapling_payment_address(
                        consensus_parameters.network_type(),
                    ),
                    address,
                ))
            }),
            // A unified address has no distinct ironwood receiver:
            // receivers (and their incoming viewing keys) are scoped
            // to the Orchard PROTOCOL, not to a pool, so the orchard
            // receiver serves both the orchard and ironwood pools.
            // See ZIP 326, <https://zips.z.cash/zip-0326>.
            ShieldedPool::Orchard | ShieldedPool::Ironwood => unified_address
                .orchard()
                .map(|address| keys::encode_orchard_receiver(consensus_parameters, address)),
        }
        .transpose()?;
        outgoing_notes
            .iter_mut()
            .filter(|note| {
                if let Ok(note_encoded_recipient) = note.encoded_recipient(consensus_parameters) {
                    encoded_address == Some(note_encoded_recipient)
                } else {
                    false
                }
            })
            .for_each(|note| {
                note.set_recipient_full_unified_address(Some(unified_address.clone()));
            });
    }

    Ok(())
}

/// Converts and adds the nullifiers from a transaction to the nullifier map.
///
/// `txid` is used instead of `transaction.txid()` due to darkside testing bug.
fn collect_nullifiers(
    nullifier_map: &mut NullifierMap,
    block_height: BlockHeight,
    txid: TxId,
    transaction: &Transaction,
) {
    let scan_target = ScanTarget::internal(block_height, txid);
    if let Some(bundle) = transaction.sapling_bundle() {
        nullifier_map.sapling.extend(
            bundle
                .shielded_spends()
                .iter()
                .map(sapling_crypto::bundle::SpendDescription::nullifier)
                .copied()
                .zip(std::iter::repeat(scan_target)),
        );
    }
    if let Some(bundle) = transaction.orchard_bundle() {
        nullifier_map.orchard.extend(
            bundle
                .actions()
                .iter()
                .map(orchard::Action::nullifier)
                .copied()
                .zip(std::iter::repeat(scan_target)),
        );
    }
    if let Some(bundle) = transaction.ironwood_bundle() {
        nullifier_map.ironwood.extend(
            bundle
                .actions()
                .iter()
                .map(orchard::Action::nullifier)
                .copied()
                .zip(std::iter::repeat(scan_target)),
        );
    }
}

/// Adds the outpoints from a transparent bundle to the outpoint map.
fn collect_outpoints<A: TransparentAuthorization>(
    outpoint_map: &mut BTreeMap<OutputId, ScanTarget>,
    txid: TxId,
    block_height: BlockHeight,
    transparent_bundle: &TransparentBundle<A>,
) {
    transparent_bundle
        .vin
        .iter()
        .map(TxIn::prevout)
        .for_each(|outpoint| {
            outpoint_map.insert(
                OutputId::from(outpoint),
                ScanTarget::internal(block_height, txid),
            );
        });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::MAIN_NETWORK;
    use zl_netutils::lightwallet_protocol::RawTransaction;

    use crate::{
        engine::{WalletBlockOps, WalletBlockParts},
        wallet::TreeBounds,
    };

    use super::*;

    const CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(1_000_000);
    const TARGET_HEIGHT: BlockHeight = BlockHeight::from_u32(500_004);

    type CoverRequestRecord = (Vec<(BlockHeight, TxId)>, Vec<BlockHeight>);
    type CoverRequestLog = Arc<Mutex<Vec<CoverRequestRecord>>>;

    /// Spawns a fetcher that answers cover requests and records their targets and timed heights.
    ///
    /// The reply serves `served` and answers every other target with `status`.
    fn cover_fetcher(
        served: Option<&TransactionRecord>,
        status: tonic::Status,
    ) -> (mpsc::UnboundedSender<FetchRequest>, CoverRequestLog) {
        let served = served.map(|served| {
            let mut data = Vec::new();
            served
                .transaction()
                .write(&mut data)
                .expect("a write to a vector does not fail");
            (served.txid(), data)
        });
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        let requests = CoverRequestLog::default();
        let log = Arc::clone(&requests);
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                let FetchRequest::Cover(reply_sender, request) = fetch_request else {
                    panic!("unexpected fetch request");
                };
                let target_replies = request
                    .targets
                    .iter()
                    .map(|(height, txid)| {
                        let reply = match &served {
                            Some((served_txid, data)) if served_txid == txid => {
                                Ok(RawTransaction {
                                    data: data.clone(),
                                    height: u64::from(*height),
                                })
                            }
                            _ => Err(status.clone()),
                        };
                        (*txid, reply)
                    })
                    .collect();
                let block_times = request
                    .timed_heights
                    .iter()
                    .map(|height| (*height, 0))
                    .collect();
                log.lock()
                    .expect("no holder of the request log panics")
                    .push((request.targets, request.timed_heights));
                let _ignore_error = reply_sender.send(Ok(CoverFetch {
                    target_replies,
                    block_times,
                }));
            }
        });

        (fetch_request_sender, requests)
    }

    fn requests(log: &CoverRequestLog) -> Vec<CoverRequestRecord> {
        log.lock()
            .expect("no holder of the request log panics")
            .clone()
    }

    async fn scan(
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        scan_target: ScanTarget,
        pending_transactions: &HashMap<TxId, Vec<u8>>,
        wallet_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    ) -> Result<ScannedTransactions, ScanError> {
        scan_transactions(
            fetch_request_sender,
            &MAIN_NETWORK,
            &HashMap::new(),
            BTreeSet::from([scan_target]),
            DecryptedNoteData::new(),
            wallet_blocks,
            &mut BTreeMap::new(),
            HashMap::new(),
            pending_transactions,
            CHAIN_HEIGHT..CHAIN_HEIGHT,
            CHAIN_HEIGHT,
        )
        .await
    }

    async fn scan_unserved(
        scan_target: ScanTarget,
        status: tonic::Status,
    ) -> Result<ScannedTransactions, ScanError> {
        scan(
            cover_fetcher(None, status).0,
            scan_target,
            &HashMap::new(),
            &BTreeMap::new(),
        )
        .await
    }

    fn pending_record() -> TransactionRecord {
        let status = ConfirmationStatus::Transmitted(TARGET_HEIGHT);
        let txid = TransactionRecord::new_for_test(TxId::from_bytes([0; 32]), status)
            .transaction()
            .txid();
        TransactionRecord::new_for_test(txid, status)
    }

    fn block_at_target_height() -> BTreeMap<BlockHeight, WalletBlock> {
        BTreeMap::from([(
            TARGET_HEIGHT,
            WalletBlock::from_parts(WalletBlockParts {
                block_height: TARGET_HEIGHT,
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds: TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 0,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            }),
        )])
    }

    fn assert_confirmed_at_target_height(scanned: &ScannedTransactions, txid: TxId) {
        assert_eq!(
            scanned.transactions.keys().copied().collect::<Vec<_>>(),
            vec![txid]
        );
        assert_eq!(
            scanned.transactions[&txid].status(),
            ConfirmationStatus::Confirmed(TARGET_HEIGHT)
        );
    }

    #[tokio::test]
    async fn an_external_target_the_server_does_not_serve_is_dropped() {
        let scan_target = ScanTarget::new(TARGET_HEIGHT, Some(TxId::from_bytes([1; 32])));

        let scanned = scan_unserved(scan_target, tonic::Status::not_found("no such transaction"))
            .await
            .expect("an unserved external target fails no scan");

        assert!(scanned.transactions.is_empty());
        assert_eq!(scanned.dropped_targets, BTreeSet::from([scan_target]));
    }

    #[tokio::test]
    async fn an_internal_target_the_server_does_not_serve_is_an_error() {
        let txid = TxId::from_bytes([1; 32]);

        assert!(matches!(
            scan_unserved(
                ScanTarget::internal(TARGET_HEIGHT, txid),
                tonic::Status::not_found("no such transaction")
            )
            .await,
            Err(ScanError::TransactionNotServed { txid: unserved, block_height, status })
                if unserved == txid
                    && block_height == TARGET_HEIGHT
                    && status.code() == tonic::Code::NotFound
        ));
    }

    #[tokio::test]
    async fn network_weather_is_an_error_and_drops_no_target() {
        for scan_target in [
            ScanTarget::new(TARGET_HEIGHT, Some(TxId::from_bytes([1; 32]))),
            ScanTarget::internal(TARGET_HEIGHT, TxId::from_bytes([1; 32])),
        ] {
            assert!(matches!(
                scan_unserved(scan_target, tonic::Status::unavailable("down")).await,
                Err(ScanError::ServerError(ServerError::RequestFailed(status)))
                    if status.code() == tonic::Code::Unavailable
            ));
        }
    }

    #[tokio::test]
    async fn a_stored_pending_transaction_is_scanned_without_a_request() {
        let pending_record = pending_record();
        let txid = pending_record.txid();
        let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
        drop(fetch_request_receiver);

        let scanned = scan(
            fetch_request_sender,
            ScanTarget::internal(TARGET_HEIGHT, txid),
            &serialize_pending_transactions([&pending_record].into_iter()),
            &block_at_target_height(),
        )
        .await
        .expect("a scan that sends no request fails on no reply");

        assert_confirmed_at_target_height(&scanned, txid);
    }

    #[tokio::test]
    async fn a_stored_pending_transaction_in_a_missing_block_requests_the_block_time_only() {
        let pending_record = pending_record();
        let txid = pending_record.txid();
        let (fetch_request_sender, log) =
            cover_fetcher(None, tonic::Status::not_found("no such transaction"));

        let scanned = scan(
            fetch_request_sender,
            ScanTarget::internal(TARGET_HEIGHT, txid),
            &serialize_pending_transactions([&pending_record].into_iter()),
            &BTreeMap::new(),
        )
        .await
        .expect("the fetcher answers the block time");

        assert_eq!(requests(&log), vec![(Vec::new(), vec![TARGET_HEIGHT])]);
        assert_confirmed_at_target_height(&scanned, txid);
    }

    #[tokio::test]
    async fn an_external_target_is_requested_even_if_stored() {
        let pending_record = pending_record();
        let txid = pending_record.txid();
        let (fetch_request_sender, log) = cover_fetcher(
            Some(&pending_record),
            tonic::Status::not_found("no such transaction"),
        );

        let scanned = scan(
            fetch_request_sender,
            ScanTarget::new(TARGET_HEIGHT, Some(txid)),
            &serialize_pending_transactions([&pending_record].into_iter()),
            &block_at_target_height(),
        )
        .await
        .expect("the fetcher serves the target");

        assert_eq!(
            requests(&log),
            vec![(vec![(TARGET_HEIGHT, txid)], Vec::new())]
        );
        assert_confirmed_at_target_height(&scanned, txid);
    }
}
