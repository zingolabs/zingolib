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
    consensus::{BlockHeight, NetworkConstants, Parameters},
    memo::Memo,
    transaction::{Transaction, TxId},
    zip32::AccountId,
};
use zingo_memo::ParsedMemo;

use crate::{
    client::{self, FetchRequest},
    keys::KeyId,
    primitives::{
        OrchardNote, OutgoingNote, OutgoingOrchardNote, OutgoingSaplingNote, OutputId, SaplingNote,
        SyncOutgoingNotes, WalletBlock, WalletNote, WalletTransaction,
    },
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

pub(crate) async fn scan_transactions<P: Parameters>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    relevant_txids: HashSet<TxId>,
    decrypted_note_data: DecryptedNoteData,
    wallet_blocks: &BTreeMap<BlockHeight, WalletBlock>,
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

        let wallet_transaction = scan_transaction(
            parameters,
            ufvks,
            transaction,
            block_height,
            &decrypted_note_data,
        )
        .unwrap();
        wallet_transactions.insert(txid, wallet_transaction);
    }

    Ok(wallet_transactions)
}

fn scan_transaction<P: Parameters>(
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transaction: Transaction,
    block_height: BlockHeight,
    decrypted_note_data: &DecryptedNoteData,
) -> Result<WalletTransaction, ()> {
    // TODO: price?
    let zip212_enforcement = zcash_primitives::transaction::components::sapling::zip212_enforcement(
        parameters,
        block_height,
    );
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

    // TODO: scan transparent bundle

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

    for encoded_memo in encoded_memos {
        match encoded_memo {
            ParsedMemo::Version0 { uas } => {
                add_recipient_unified_address(parameters, uas.clone(), &mut outgoing_sapling_notes);
                add_recipient_unified_address(parameters, uas, &mut outgoing_orchard_notes);
            }
            _ => panic!(
                "memo version not supported. please ensure that your software is up-to-date."
            ),
        }
    }
    Ok(WalletTransaction::from_parts(
        transaction,
        block_height,
        sapling_notes,
        orchard_notes,
        outgoing_sapling_notes,
        outgoing_orchard_notes,
    ))
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
                *position,
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
    P: Parameters + NetworkConstants,
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
