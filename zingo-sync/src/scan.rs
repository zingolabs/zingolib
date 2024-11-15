use std::{
    cmp,
    collections::{BTreeMap, HashMap, HashSet},
};

use incrementalmerkletree::{Marking, Position, Retention};
use orchard::{
    note_encryption::{CompactAction, OrchardDomain},
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action, Address,
};
use sapling_crypto::{
    bundle::{GrothProofBytes, OutputDescription},
    note_encryption::{CompactOutputDescription, SaplingDomain},
    Node,
};
use tokio::sync::mpsc;
use zcash_client_backend::{
    data_api::scanning::ScanRange,
    proto::compact_formats::{CompactBlock, CompactOrchardAction, CompactSaplingOutput, CompactTx},
};
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_note_encryption::{BatchDomain, Domain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkConstants, NetworkUpgrade, Parameters},
    legacy::TransparentAddress,
    memo::{Memo, MemoBytes},
    transaction::{Transaction, TxId},
};
use zingo_memo::ParsedMemo;

use crate::{
    client::{self, get_compact_block_range, FetchRequest},
    keys::{KeyId, ScanningKeyOps, ScanningKeys},
    primitives::{
        NullifierMap, OrchardNote, OutgoingData, OutputId, SaplingNote, SyncNote, WalletBlock,
        WalletTransaction,
    },
    witness::ShardTreeData,
};

use self::runners::{BatchRunners, DecryptedOutput};

pub(crate) mod runners;
pub(crate) mod workers;

struct InitialScanData {
    previous_block: Option<WalletBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData {
    async fn new<P>(
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        parameters: &P,
        first_block: &CompactBlock,
        previous_wallet_block: Option<WalletBlock>,
    ) -> Result<Self, ()>
    where
        P: Parameters + Sync + Send + 'static,
    {
        // gets initial tree size from previous block if available
        // otherwise, from first block if available
        // otherwise, fetches frontiers from server
        let (sapling_initial_tree_size, orchard_initial_tree_size) =
            if let Some(prev) = &previous_wallet_block {
                (
                    prev.sapling_commitment_tree_size(),
                    prev.orchard_commitment_tree_size(),
                )
            } else if let Some(chain_metadata) = &first_block.chain_metadata {
                // calculate initial tree size by subtracting number of outputs in block from the blocks final tree size
                let sapling_output_count: u32 = first_block
                    .vtx
                    .iter()
                    .map(|tx| tx.outputs.len())
                    .sum::<usize>()
                    .try_into()
                    .expect("Sapling output count cannot exceed a u32");
                let orchard_output_count: u32 = first_block
                    .vtx
                    .iter()
                    .map(|tx| tx.actions.len())
                    .sum::<usize>()
                    .try_into()
                    .expect("Sapling output count cannot exceed a u32");

                (
                    chain_metadata
                        .sapling_commitment_tree_size
                        .checked_sub(sapling_output_count)
                        .unwrap(),
                    chain_metadata
                        .orchard_commitment_tree_size
                        .checked_sub(orchard_output_count)
                        .unwrap(),
                )
            } else {
                let sapling_activation_height = parameters
                    .activation_height(NetworkUpgrade::Sapling)
                    .expect("should have some sapling activation height");

                match first_block.height().cmp(&sapling_activation_height) {
                    cmp::Ordering::Greater => {
                        let frontiers =
                            client::get_frontiers(fetch_request_sender, first_block.height() - 1)
                                .await
                                .unwrap();
                        (
                            frontiers.final_sapling_tree().tree_size() as u32,
                            frontiers.final_orchard_tree().tree_size() as u32,
                        )
                    }
                    cmp::Ordering::Equal => (0, 0),
                    cmp::Ordering::Less => panic!("pre-sapling not supported!"),
                }
            };

        Ok(InitialScanData {
            previous_block: previous_wallet_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

#[allow(dead_code)]
struct ScanData {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) relevant_txids: HashSet<TxId>,
    pub(crate) decrypted_note_data: DecryptedNoteData,
    pub(crate) shard_tree_data: ShardTreeData,
}

pub(crate) struct ScanResults {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) wallet_transactions: HashMap<TxId, WalletTransaction>,
    pub(crate) shard_tree_data: ShardTreeData,
}

struct DecryptedNoteData {
    sapling_nullifiers_and_positions: HashMap<OutputId, (sapling_crypto::Nullifier, Position)>,
    orchard_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
}

impl DecryptedNoteData {
    fn new() -> Self {
        DecryptedNoteData {
            sapling_nullifiers_and_positions: HashMap::new(),
            orchard_nullifiers_and_positions: HashMap::new(),
        }
    }
}

impl Default for DecryptedNoteData {
    fn default() -> Self {
        Self::new()
    }
}

// scans a given range and returns all data relevant to the specified keys
// `previous_wallet_block` is the wallet block with height [scan_range.start - 1]
pub(crate) async fn scan<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys,
    scan_range: ScanRange,
    previous_wallet_block: Option<WalletBlock>,
) -> Result<ScanResults, ()>
where
    P: Parameters + Sync + Send + 'static,
{
    let compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender.clone(),
        parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
        previous_wallet_block,
    )
    .await
    .unwrap();

    let scan_data =
        scan_compact_blocks(compact_blocks, parameters, scanning_keys, initial_scan_data).unwrap();

    let ScanData {
        nullifiers,
        wallet_blocks,
        relevant_txids,
        decrypted_note_data,
        shard_tree_data,
    } = scan_data;

    let wallet_transactions = scan_transactions(
        fetch_request_sender,
        parameters,
        scanning_keys,
        relevant_txids,
        decrypted_note_data,
        &wallet_blocks,
    )
    .await
    .unwrap();

    Ok(ScanResults {
        nullifiers,
        wallet_blocks,
        wallet_transactions,
        shard_tree_data,
    })
}

fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    parameters: &P,
    scanning_keys: &ScanningKeys,
    initial_scan_data: InitialScanData,
) -> Result<ScanData, ()>
where
    P: Parameters + Sync + Send + 'static,
{
    check_continuity(&compact_blocks, initial_scan_data.previous_block.as_ref()).unwrap();

    let mut runners = trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    let mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock> = BTreeMap::new();
    let mut nullifiers = NullifierMap::new();
    let mut relevant_txids: HashSet<TxId> = HashSet::new();
    let mut decrypted_note_data = DecryptedNoteData::new();
    let mut shard_tree_data = ShardTreeData::new(
        Position::from(u64::from(initial_scan_data.sapling_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.orchard_initial_tree_size)),
    );
    let mut sapling_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_tree_size = initial_scan_data.orchard_initial_tree_size;
    for block in &compact_blocks {
        let mut transactions = block.vtx.iter().peekable();
        while let Some(transaction) = transactions.next() {
            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            let incoming_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());

            // gather the txids of all transactions relevant to the wallet
            // the edge case of transactions that this capability created but did not receive change
            // or create outgoing data is handled when the nullifiers are added and linked
            incoming_sapling_outputs.iter().for_each(|(output_id, _)| {
                relevant_txids.insert(output_id.txid());
            });
            incoming_orchard_outputs.iter().for_each(|(output_id, _)| {
                relevant_txids.insert(output_id.txid());
            });
            // TODO: add outgoing outputs to relevant txids

            collect_nullifiers(&mut nullifiers, block.height(), transaction).unwrap();

            shard_tree_data.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_sapling_outputs,
                )
                .unwrap(),
            );
            shard_tree_data.orchard_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.actions,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_orchard_outputs,
                )
                .unwrap(),
            );

            calculate_nullifiers_and_positions(
                sapling_tree_size,
                scanning_keys.sapling(),
                &incoming_sapling_outputs,
                &mut decrypted_note_data.sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                orchard_tree_size,
                scanning_keys.orchard(),
                &incoming_orchard_outputs,
                &mut decrypted_note_data.orchard_nullifiers_and_positions,
            );

            sapling_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }

        let wallet_block = WalletBlock::from_parts(
            block.height(),
            block.hash(),
            block.prev_hash(),
            block.time,
            block.vtx.iter().map(|tx| tx.txid()).collect(),
            sapling_tree_size,
            orchard_tree_size,
        );

        check_tree_size(block, &wallet_block).unwrap();

        wallet_blocks.insert(wallet_block.block_height(), wallet_block);
    }
    // TODO: map nullifiers

    Ok(ScanData {
        nullifiers,
        wallet_blocks,
        relevant_txids,
        decrypted_note_data,
        shard_tree_data,
    })
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
) -> Result<BatchRunners<(), ()>, ()>
where
    P: Parameters + Send + 'static,
{
    // TODO: add outgoing decryption

    let mut runners = BatchRunners::<(), ()>::for_keys(100, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    Ok(runners)
}

// checks height and hash continuity of a batch of compact blocks.
// takes the last wallet compact block of the adjacent lower scan range, if available.
fn check_continuity(
    compact_blocks: &[CompactBlock],
    previous_compact_block: Option<&WalletBlock>,
) -> Result<(), ()> {
    let mut prev_height: Option<BlockHeight> = None;
    let mut prev_hash: Option<BlockHash> = None;

    if let Some(prev) = previous_compact_block {
        prev_height = Some(prev.block_height());
        prev_hash = Some(prev.block_hash());
    }

    for block in compact_blocks {
        if let Some(prev_height) = prev_height {
            if block.height() != prev_height + 1 {
                panic!("height discontinuity");
            }
        }

        if let Some(prev_hash) = prev_hash {
            if block.prev_hash() != prev_hash {
                panic!("hash discontinuity");
            }
        }

        prev_height = Some(block.height());
        prev_hash = Some(block.hash());
    }

    Ok(())
}

fn check_tree_size(compact_block: &CompactBlock, wallet_block: &WalletBlock) -> Result<(), ()> {
    if let Some(chain_metadata) = &compact_block.chain_metadata {
        if chain_metadata.sapling_commitment_tree_size
            != wallet_block.sapling_commitment_tree_size()
        {
            panic!("sapling tree size is incorrect!")
        }
        if chain_metadata.orchard_commitment_tree_size
            != wallet_block.orchard_commitment_tree_size()
        {
            panic!("orchard tree size is incorrect!")
        }
    }

    Ok(())
}

// calculates nullifiers and positions of incoming decrypted outputs for a given compact transaction and insert into hash map
// `tree_size` is the tree size of the corresponding shielded pool up to - and not including - the compact transaction
// being processed
fn calculate_nullifiers_and_positions<D, K, Nf>(
    tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: ScanningKeyOps<D, Nf>,
{
    incoming_decrypted_outputs
        .iter()
        .for_each(|(output_id, incoming_output)| {
            let position = Position::from(u64::from(
                tree_size + u32::try_from(output_id.output_index()).unwrap(),
            ));
            let key = keys
                .get(&incoming_output.ivk_tag)
                .expect("key should be available as it was used to decrypt output");
            let nullifier = key
                .nf(&incoming_output.note, position)
                .expect("only fvks currently supported");
            nullifiers_and_positions.insert(*output_id, (nullifier, position));
        });
}

// TODO: unify sapling and orchard leaf and retention fns
// calculates the sapling note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_sapling_leaves_and_retentions<D: Domain>(
    outputs: &[CompactSaplingOutput],
    block_height: BlockHeight,
    last_outputs_in_block: bool,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(Node, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();

    if outputs.is_empty() {
        Ok(Vec::new())
    } else {
        let last_output_index = outputs.len() - 1;

        let leaves_and_retentions = outputs
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactOutputDescription::try_from(output).unwrap().cmu;
                let leaf = sapling_crypto::Node::from_cmu(&note_commitment);

                let last_output_in_block: bool =
                    last_outputs_in_block && output_index == last_output_index;
                let decrypted: bool = incoming_output_indices.contains(&output_index);
                let retention = match (decrypted, last_output_in_block) {
                    (is_marked, true) => Retention::Checkpoint {
                        id: block_height,
                        marking: if is_marked {
                            Marking::Marked
                        } else {
                            Marking::None
                        },
                    },
                    (true, false) => Retention::Marked,
                    (false, false) => Retention::Ephemeral,
                };

                (leaf, retention)
            })
            .collect();

        Ok(leaves_and_retentions)
    }
}
// calculates the orchard note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_orchard_leaves_and_retentions<D: Domain>(
    actions: &[CompactOrchardAction],
    block_height: BlockHeight,
    last_outputs_in_block: bool,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(MerkleHashOrchard, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();

    if actions.is_empty() {
        Ok(Vec::new())
    } else {
        let last_output_index = actions.len() - 1;

        let leaves_and_retentions = actions
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactAction::try_from(output).unwrap().cmx();
                let leaf = MerkleHashOrchard::from_cmx(&note_commitment);

                let last_output_in_block: bool =
                    last_outputs_in_block && output_index == last_output_index;
                let decrypted: bool = incoming_output_indices.contains(&output_index);
                let retention = match (decrypted, last_output_in_block) {
                    (is_marked, true) => Retention::Checkpoint {
                        id: block_height,
                        marking: if is_marked {
                            Marking::Marked
                        } else {
                            Marking::None
                        },
                    },
                    (true, false) => Retention::Marked,
                    (false, false) => Retention::Ephemeral,
                };

                (leaf, retention)
            })
            .collect();

        Ok(leaves_and_retentions)
    }
}

// converts and adds the nullifiers from a compact transaction to the nullifier map
fn collect_nullifiers(
    nullifier_map: &mut NullifierMap,
    block_height: BlockHeight,
    transaction: &CompactTx,
) -> Result<(), ()> {
    transaction
        .spends
        .iter()
        .map(|spend| sapling_crypto::Nullifier::from_slice(spend.nf.as_slice()).unwrap())
        .for_each(|nullifier| {
            nullifier_map
                .sapling_mut()
                .insert(nullifier, (block_height, transaction.txid()));
        });
    transaction
        .actions
        .iter()
        .map(|action| {
            orchard::note::Nullifier::from_bytes(action.nullifier.as_slice().try_into().unwrap())
                .unwrap()
        })
        .for_each(|nullifier| {
            nullifier_map
                .orchard_mut()
                .insert(nullifier, (block_height, transaction.txid()));
        });
    Ok(())
}

async fn scan_transactions<P: Parameters>(
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
    outgoing_data: &mut [OutgoingData],
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

                ParsedMemo::Version1 {
                    uas,
                    rejection_address_indexes,
                } => {
                    dbg!(uas);
                    dbg!(rejection_address_indexes);
                    todo!("Handle this new case!")
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
    outgoing_data: &mut [OutgoingData],
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
    orchard_address: &Address,
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
use base58::ToBase58;
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
    let h2 = <sha2::Sha256 as sha2::Digest>::digest(h1);
    h2.to_vec()
}
//
// TEMP
//
