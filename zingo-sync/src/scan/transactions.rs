use std::collections::{BTreeMap, HashMap, HashSet};

use base58::ToBase58;
use incrementalmerkletree::Position;
use orchard::{
    note_encryption::OrchardDomain,
    primitives::redpallas::{Signature, SpendAuth},
    Action,
};
use sapling_crypto::{
    bundle::{GrothProofBytes, OutputDescription},
    note_encryption::SaplingDomain,
};
use tokio::sync::mpsc;

use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_note_encryption::{BatchDomain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkConstants, Parameters},
    legacy::TransparentAddress,
    memo::{Memo, MemoBytes},
    transaction::{Transaction, TxId},
};
use zingo_memo::ParsedMemo;

use crate::{
    client::{self, FetchRequest},
    keys::{ScanningKeyOps as _, ScanningKeys},
    primitives::{
        OrchardNote, OutgoingData, OutputId, SaplingNote, SyncNote, WalletBlock, WalletTransaction,
    },
};

use super::DecryptedNoteData;

pub(crate) async fn scan_transactions<P: Parameters>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys,
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
            scanning_keys,
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
    scanning_keys: &ScanningKeys,
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
    let mut outgoing_data: Vec<OutgoingData> = Vec::new();

    if let Some(bundle) = transaction.sapling_bundle() {
        let sapling_keys: Vec<sapling_crypto::keys::PreparedIncomingViewingKey> = scanning_keys
            .sapling()
            .iter()
            .map(|(_, key)| key.prepare())
            .collect();
        let sapling_outputs: Vec<(SaplingDomain, OutputDescription<GrothProofBytes>)> = bundle
            .shielded_outputs()
            .iter()
            .map(|output| (SaplingDomain::new(zip212_enforcement), output.clone()))
            .collect();

        scan_notes::<SaplingDomain, OutputDescription<GrothProofBytes>, SaplingNote>(
            &mut sapling_notes,
            transaction.txid(),
            &sapling_keys,
            &sapling_outputs,
            &decrypted_note_data.sapling_nullifiers_and_positions,
        )
        .unwrap();
    }

    if let Some(bundle) = transaction.orchard_bundle() {
        let orchard_keys: Vec<orchard::keys::PreparedIncomingViewingKey> = scanning_keys
            .orchard()
            .iter()
            .map(|(_, key)| key.prepare())
            .collect();
        let orchard_actions: Vec<(OrchardDomain, Action<Signature<SpendAuth>>)> = bundle
            .actions()
            .iter()
            .map(|action| (OrchardDomain::for_action(action), action.clone()))
            .collect();

        scan_notes::<OrchardDomain, Action<Signature<SpendAuth>>, OrchardNote>(
            &mut orchard_notes,
            transaction.txid(),
            &orchard_keys,
            &orchard_actions,
            &decrypted_note_data.orchard_nullifiers_and_positions,
        )
        .unwrap();
    }

    parse_encoded_memos(parameters, &sapling_notes, &mut outgoing_data).unwrap();
    parse_encoded_memos(parameters, &orchard_notes, &mut outgoing_data).unwrap();

    Ok(WalletTransaction::from_parts(
        transaction.txid(),
        block_height,
        sapling_notes,
        orchard_notes,
    ))
}

fn scan_notes<D, Op, N>(
    wallet_notes: &mut Vec<N::WalletNote>,
    txid: TxId,
    ivks: &[D::IncomingViewingKey],
    outputs: &[(D, Op)],
    nullifiers_and_positions: &HashMap<OutputId, (N::Nullifier, Position)>,
) -> Result<(), ()>
where
    D: BatchDomain,
    D::Memo: AsRef<[u8]>,
    Op: ShieldedOutput<D, ENC_CIPHERTEXT_SIZE>,
    N: SyncNote<ZcashNote = D::Note, Memo = Memo>,
{
    for (output_index, output) in zcash_note_encryption::batch::try_note_decryption(ivks, outputs)
        .into_iter()
        .enumerate()
    {
        if let Some(((note, _, memo_bytes), _)) = output {
            let output_id = OutputId::from_parts(txid, output_index);
            let (nullifier, position) = nullifiers_and_positions.get(&output_id).unwrap();
            let memo_bytes = MemoBytes::from_bytes(memo_bytes.as_ref()).unwrap();
            let memo =
                Memo::try_from(memo_bytes.clone()).unwrap_or_else(|_| Memo::Future(memo_bytes));

            let wallet_note = N::from_parts(output_id, note, *nullifier, *position, memo);
            wallet_notes.push(wallet_note);
        }
    }

    Ok(())
}

fn parse_encoded_memos<P, N>(
    parameters: &P,
    wallet_notes: &[N],
    outgoing_data: &mut Vec<OutgoingData>,
) -> Result<(), ()>
where
    P: Parameters,
    N: SyncNote<Memo = Memo>,
{
    if outgoing_data.is_empty() {
        return Ok(());
    }

    for note in wallet_notes {
        if let Memo::Arbitrary(ref encoded_memo_bytes) = note.memo() {
            let encoded_memo = zingo_memo::parse_zingo_memo(*encoded_memo_bytes.as_ref()).unwrap();

            match encoded_memo {
                ParsedMemo::Version0 { uas } => {
                    add_recipient_unified_address(parameters, uas, outgoing_data);
                }
                _ => panic!(
                    "memo version not supported. please ensure that your software is up-to-date."
                ),
            }
        }
    }

    Ok(())
}

fn add_recipient_unified_address<P>(
    parameters: &P,
    unified_addresses: Vec<UnifiedAddress>,
    outgoing_data: &mut Vec<OutgoingData>,
) where
    P: Parameters + NetworkConstants,
{
    for ua in unified_addresses {
        let ua_receivers = [
            encode_orchard_receiver(parameters, ua.orchard().unwrap()).unwrap(),
            encode_payment_address(
                parameters.hrp_sapling_payment_address(),
                ua.sapling().unwrap(),
            ),
            address_from_pubkeyhash(parameters, ua.transparent().unwrap()),
            ua.encode(parameters),
        ];
        outgoing_data
            .iter_mut()
            .filter(|od| ua_receivers.contains(&od.recipient_address))
            .for_each(|od| od.recipient_ua = Some(ua.clone()))
    }
}

// TODO: remove temp encoding and replace string with address enums in outgoing data
//
// TEMP
//
//
fn encode_orchard_receiver<P: Parameters>(
    parameters: &P,
    orchard_address: &orchard::Address,
) -> Result<String, ()> {
    Ok(zcash_address::unified::Encoding::encode(
        &<zcash_address::unified::Address as zcash_address::unified::Encoding>::try_from_items(
            vec![zcash_address::unified::Receiver::Orchard(
                orchard_address.to_raw_address_bytes(),
            )],
        )
        .unwrap(),
        &parameters.network_type(),
    ))
}
fn address_from_pubkeyhash<P: NetworkConstants>(
    parameters: &P,
    transparent_address: &TransparentAddress,
) -> String {
    match transparent_address {
        TransparentAddress::PublicKeyHash(hash) => {
            hash.to_base58check(&parameters.b58_pubkey_address_prefix(), &[])
        }
        TransparentAddress::ScriptHash(hash) => {
            hash.to_base58check(&parameters.b58_script_address_prefix(), &[])
        }
    }
}
/// A trait for converting a [u8] to base58 encoded string.
trait ToBase58Check {
    /// Converts a value of `self` to a base58 value, returning the owned string.
    /// The version is a coin-specific prefix that is added.
    /// The suffix is any bytes that we want to add at the end (like the "iscompressed" flag for
    /// Secret key encoding)
    fn to_base58check(&self, version: &[u8], suffix: &[u8]) -> String;
}
impl ToBase58Check for [u8] {
    fn to_base58check(&self, version: &[u8], suffix: &[u8]) -> String {
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(version);
        payload.extend_from_slice(self);
        payload.extend_from_slice(suffix);

        let checksum = double_sha256(&payload);
        payload.append(&mut checksum[..4].to_vec());
        payload.to_base58()
    }
}
/// Sha256(Sha256(value))
fn double_sha256(payload: &[u8]) -> Vec<u8> {
    let h1 = <sha2::Sha256 as sha2::Digest>::digest(payload);
    let h2 = <sha2::Sha256 as sha2::Digest>::digest(&h1);
    h2.to_vec()
}
//
// TEMP
//
