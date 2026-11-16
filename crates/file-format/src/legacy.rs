use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt};
use orchard::tree::MerkleHashOrchard;
use prost::Message;

use incrementalmerkletree::{Address, Hashable, Level, Position, witness::IncrementalWitness};
use lightwallet_protocol::CompactBlock;
use shardtree::{
    LocatedPrunableTree, ShardTree,
    store::{Checkpoint, ShardStore as _, memory::MemoryShardStore},
};
use zcash_client_backend::serialization::shardtree::read_shard;
use zcash_encoding::{CompactSize, Optional, Vector};
use zcash_primitives::{
    merkle_tree::{HashSer, read_commitment_tree, read_incremental_witness},
    transaction::TxId,
};
use zcash_protocol::{
    consensus::BlockHeight,
    memo::{Memo, MemoBytes},
};
use zl_common::shardtree_ext::ShardTreeExt as _;
use zl_common::status::ConfirmationStatus;

use zl_common::keys::KeyError;
use zl_common::serialization::{ReadableWriteable, read_version};

use crate::strings;

pub(crate) mod keys;

use keys::WalletCapability;

type DatedWitness<H> = (IncrementalWitness<H, COMMITMENT_TREE_LEVELS>, BlockHeight);

const MEMO_SIZE: usize = 512;
const SAPLING_DISCARDED_PREFIX_SIZE: usize = 169;
const ORCHARD_DISCARDED_PREFIX_SIZE: usize = 96;

#[derive(Debug, thiserror::Error)]
enum DecodeError {
    #[error("stored orchard nullifier is not a canonical field element")]
    OrchardNullifier,
    #[error("stored witness cache is taller than its top height")]
    WitnessCacheHeight,
    #[error("wallet holds no sapling viewing key for its stored sapling note")]
    SaplingViewingKey(#[source] KeyError),
    #[error("stored sapling note diversifier does not map to a jubjub point")]
    SaplingDiversifier,
    #[error("stored sapling rseed is not a canonical jubjub scalar")]
    SaplingRseedScalar,
    #[error("stored sapling rseed type {0} is unknown")]
    SaplingRseedType(u8),
    #[error("Bad Nullifier")]
    OrchardRho,
    #[error("Nullifier not for note")]
    OrchardRandomSeed,
    #[error("wallet holds no orchard viewing key for its stored orchard note")]
    OrchardViewingKey(#[source] KeyError),
    #[error("Invalid note")]
    OrchardNote,
    #[error("stored transparent output address is not transparent")]
    TransparentAddress,
    #[error("Couldn't create memo")]
    Memo(#[source] zcash_protocol::memo::Error),
    #[error("Can't read {0} because of incorrect version")]
    Version(&'static str),
    #[error("Bad confirmation status")]
    ConfirmationStatus,
    #[error("Bad download option {0}")]
    MemoDownloadOption(u8),
    #[error("error reading TreeState: expected boolean value, found {0}")]
    TreeState(u8),
    #[error("parent node in root has level 0 relative to root address: {0:?}")]
    ShardRoot(Address),
}

impl From<DecodeError> for io::Error {
    fn from(error: DecodeError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, error)
    }
}

fn orchard_nullifier(bytes: &[u8; 32]) -> Result<orchard::note::Nullifier, DecodeError> {
    orchard::note::Nullifier::from_bytes(bytes)
        .into_option()
        .ok_or(DecodeError::OrchardNullifier)
}

fn witness_height(top_height: u64, depth: usize) -> Result<BlockHeight, DecodeError> {
    u32::try_from(top_height)
        .ok()
        .zip(u32::try_from(depth).ok())
        .and_then(|(top_height, depth)| top_height.checked_sub(depth))
        .map(BlockHeight::from)
        .ok_or(DecodeError::WitnessCacheHeight)
}

fn sapling_rseed(note_type: u8, bytes: [u8; 32]) -> Result<sapling_crypto::Rseed, DecodeError> {
    match note_type {
        1 => jubjub::Fr::from_bytes(&bytes)
            .into_option()
            .map(sapling_crypto::Rseed::BeforeZip212)
            .ok_or(DecodeError::SaplingRseedScalar),
        2 => Ok(sapling_crypto::Rseed::AfterZip212(bytes)),
        unknown => Err(DecodeError::SaplingRseedType(unknown)),
    }
}

fn memo(bytes: &[u8]) -> Result<Memo, DecodeError> {
    let bytes = MemoBytes::from_bytes(bytes).map_err(DecodeError::Memo)?;
    Ok(Memo::try_from(bytes.clone()).unwrap_or(Memo::Future(bytes)))
}

fn read_memo<R: Read>(mut reader: R) -> io::Result<Memo> {
    let mut bytes = [0u8; MEMO_SIZE];
    reader.read_exact(&mut bytes)?;
    Ok(memo(&bytes)?)
}

fn read_witness_cache<H: HashSer + Hashable + Clone, R: Read>(
    mut reader: R,
    dated_witnesses: Option<&mut Vec<DatedWitness<H>>>,
) -> io::Result<Option<Position>> {
    let witnesses = Vector::read(&mut reader, |r| read_incremental_witness(r))?;
    let top_height = reader.read_u64::<LittleEndian>()?;
    let position = witnesses.last().map(IncrementalWitness::witnessed_position);
    let newest = witnesses.len().saturating_sub(1);
    let dated = witnesses
        .into_iter()
        .enumerate()
        .map(|(index, witness)| Ok((witness, witness_height(top_height, newest - index)?)))
        .collect::<io::Result<Vec<_>>>()?;
    if let Some(dated_witnesses) = dated_witnesses {
        dated_witnesses.extend(dated);
    }
    Ok(position)
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, PartialEq)]
pub struct BlockData {
    /// TODO: Add Doc Comment Here!
    pub(crate) ecb: Vec<u8>,
    /// TODO: Add Doc Comment Here!
    pub height: u64,
}

impl BlockData {
    pub(crate) fn new_with(height: u64, hash: &[u8]) -> Self {
        let hash = hash.iter().copied().rev().collect::<Vec<_>>();

        let cb = CompactBlock {
            hash,
            ..Default::default()
        };

        let mut ecb = vec![];
        cb.encode(&mut ecb)
            .expect("a Vec<u8> grows to fit any encoded message");

        Self { ecb, height }
    }

    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let height = reader.read_i32::<LittleEndian>()? as u64;

        let mut hash_bytes = [0; 32];
        reader.read_exact(&mut hash_bytes)?;
        hash_bytes.reverse();

        // We don't need this, but because of a quirk, the version is stored later, so we can't actually
        // detect the version here. So we write an empty tree and read it back here
        let tree: sapling_crypto::CommitmentTree = read_commitment_tree(&mut reader)?;
        let _tree = if tree.size() == 0 { None } else { Some(tree) };

        let version = reader.read_u64::<LittleEndian>()?;

        let ecb = if version <= 11 {
            vec![]
        } else {
            Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?
        };

        if ecb.is_empty() {
            Ok(BlockData::new_with(height, &hash_bytes))
        } else {
            Ok(BlockData { ecb, height })
        }
    }
}

/// `HashMap` of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a `RwLock`, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMap {
    /// TODO: Doc-comment!
    pub transaction_records_by_id: TransactionRecordsById,
}

impl TxMap {
    /// TODO: Doc-comment!
    pub fn serialized_version() -> u64 {
        23
    }

    /// TODO: Doc-comment!
    pub fn read_old<R: Read>(
        mut reader: R,
        wallet_capability: &WalletCapability,
    ) -> io::Result<Self> {
        // Note, witness_trees will be Some(x) if the wallet has spend capability
        // so this check is a very un-ergonomic way of checking if the wallet
        // can spend.
        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let txs = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionRecord::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?,
            ))
        })?;

        let map = TransactionRecordsById::from_map(txs);

        if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by_key(|(_witness, height)| *height);
            let sap_tree = &mut witness_trees
                .as_mut()
                .expect("old witnesses are collected only when the capability has witness trees")
                .witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                // Witnesses arrive height-sorted, so `NotAboveNewest` here
                // means a second witness at an already-checkpointed height:
                // benign, the checkpoint exists. Classified rather than
                // discarded so a genuinely skipped checkpoint can never
                // hide in a boolean again.
                let _ = sap_tree.append_checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by_key(|(_witness, height)| *height);
            let orch_tree = &mut witness_trees
                .as_mut()
                .expect("old witnesses are collected only when the capability has witness trees")
                .witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                // As for sapling above: height-sorted, duplicates benign.
                let _ = orch_tree.append_checkpoint(height).expect("infallible");
            }
        }

        Ok(Self {
            transaction_records_by_id: map,
        })
    }

    /// TODO: Doc-comment!
    #[allow(unused_assignments)]
    pub fn read<R: Read>(mut reader: R, wallet_capability: &WalletCapability) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(DecodeError::Version("wallettxns").into());
        }

        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let map: HashMap<_, _> = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionRecord::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?,
            ))
        })?;

        let _mempool: Vec<(TxId, TransactionRecord)> = if version <= 20 {
            Vector::read_collected_mut(&mut reader, |r| {
                let mut txid_bytes = [0u8; 32];
                r.read_exact(&mut txid_bytes)?;
                let transaction_metadata =
                    TransactionRecord::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?;

                Ok((TxId::from_bytes(txid_bytes), transaction_metadata))
            })?
        } else {
            vec![]
        };

        if version >= 22 {
            witness_trees = Optional::read(reader, |r| WitnessTrees::read(r))?;
        } else if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by_key(|(_witness, height)| *height);
            let sap_tree = &mut witness_trees
                .as_mut()
                .expect("old witnesses are collected only when the capability has witness trees")
                .witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                // Witnesses arrive height-sorted, so `NotAboveNewest` here
                // means a second witness at an already-checkpointed height:
                // benign, the checkpoint exists. Classified rather than
                // discarded so a genuinely skipped checkpoint can never
                // hide in a boolean again.
                let _ = sap_tree.append_checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by_key(|(_witness, height)| *height);
            let orch_tree = &mut witness_trees
                .as_mut()
                .expect("old witnesses are collected only when the capability has witness trees")
                .witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                // As for sapling above: height-sorted, duplicates benign.
                let _ = orch_tree.append_checkpoint(height).expect("infallible");
            }
        }

        Ok(Self {
            transaction_records_by_id: TransactionRecordsById::from_map(map),
        })
    }
}

/// A convenience wrapper, to impl behavior on.
pub struct TransactionRecordsById(pub HashMap<TxId, TransactionRecord>);

impl TransactionRecordsById {
    /// Constructs a `TransactionRecordsById` from a `HashMap`
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }
}

///  Everything (SOMETHING) about a transaction
#[allow(dead_code)]
pub struct TransactionRecord {
    /// the relationship of the transaction to the blockchain. can be either Broadcast (to mempool}, or Confirmed.
    pub status: zl_common::status::ConfirmationStatus,
    /// Timestamp of Tx. Added in v4
    pub datetime: u64,
    /// Txid of this transaction. It's duplicated here (It is also the Key in the `HashMap` that points to this
    /// `WalletTx` in `LightWallet::txs`)
    pub txid: TxId,
    /// List of all nullifiers spent by this wallet in this Tx.
    pub spent_sapling_nullifiers: Vec<sapling_crypto::Nullifier>,
    /// List of all nullifiers spent by this wallet in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<orchard::note::Nullifier>,
    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<SaplingNote>,
    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<OrchardNote>,
    /// List of all Utxos by this wallet received in this Tx. Some of these might be change notes
    pub transparent_outputs: Vec<TransparentOutput>,
    /// Total amount of transparent funds that belong to us that were spent by this wallet in this Tx.
    pub total_transparent_value_spent: u64,
    /// Total value of all the sapling nullifiers that were spent by this wallet in this Tx
    pub total_sapling_value_spent: u64,
    /// Total value of all the orchard nullifiers that were spent by this wallet in this Tx
    pub total_orchard_value_spent: u64,
    /// All outgoing sends
    pub outgoing_tx_data: Vec<OutgoingTxData>,
    /// Price of Zec when this Tx was created
    pub price: Option<f64>,
}

impl TransactionRecord {
    /// TODO: Add Doc Comment Here!
    #[allow(clippy::type_complexity)]
    pub fn read<R: Read>(
        mut reader: R,
        (wallet_capability, mut trees): (
            &WalletCapability,
            Option<&mut (
                Vec<(
                    IncrementalWitness<sapling_crypto::Node, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
                Vec<(
                    IncrementalWitness<MerkleHashOrchard, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
            )>,
        ),
    ) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;

        let block = BlockHeight::from_u32(reader.read_i32::<LittleEndian>()? as u32);

        let pending = if version <= 20 {
            false
        } else {
            reader.read_u8()? == 1
        };

        let datetime = if version >= 4 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        let mut transaction_id_bytes = [0u8; 32];
        reader.read_exact(&mut transaction_id_bytes)?;

        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let sapling_notes = zcash_encoding::Vector::read_collected_mut(&mut reader, |r| {
            SaplingNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.0)))
        })?;
        let orchard_notes = if version > 22 {
            zcash_encoding::Vector::read_collected_mut(&mut reader, |r| {
                OrchardNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.1)))
            })?
        } else {
            vec![]
        };

        let utxos = zcash_encoding::Vector::read(&mut reader, |r| TransparentOutput::read(r))?;

        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        let outgoing_metadata = match version {
            ..24 => zcash_encoding::Vector::read(&mut reader, |r| OutgoingTxData::read_old(r))?,
            24.. => zcash_encoding::Vector::read(&mut reader, |r| OutgoingTxData::read(r))?,
        };
        let _full_tx_scanned = reader.read_u8()? > 0;

        let zec_price = if version <= 4 {
            None
        } else {
            zcash_encoding::Optional::read(
                &mut reader,
                byteorder::ReadBytesExt::read_f64::<LittleEndian>,
            )?
        };

        let spent_sapling_nullifiers = if version <= 5 {
            vec![]
        } else {
            zcash_encoding::Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(sapling_crypto::Nullifier(n))
            })?
        };

        let spent_orchard_nullifiers = if version <= 21 {
            vec![]
        } else {
            zcash_encoding::Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(orchard_nullifier(&n)?)
            })?
        };
        let status = if pending {
            ConfirmationStatus::Transmitted(block)
        } else {
            ConfirmationStatus::Confirmed(block)
        };

        Ok(Self {
            status,
            datetime,
            txid: transaction_id,
            sapling_notes,
            orchard_notes,
            transparent_outputs: utxos,
            spent_sapling_nullifiers,
            spent_orchard_nullifiers,
            total_transparent_value_spent,
            total_sapling_value_spent,
            total_orchard_value_spent,
            outgoing_tx_data: outgoing_metadata,
            price: zec_price,
        })
    }
}

const SAPLING_NOTE_VERSION: u8 = 1;

fn read_sapling_note<R: Read>(
    mut reader: R,
    diversifier: sapling_crypto::Diversifier,
    wallet_capability: &WalletCapability,
) -> io::Result<sapling_crypto::Note> {
    let _version = read_version(
        &mut reader,
        std::any::type_name::<sapling_crypto::Note>(),
        SAPLING_NOTE_VERSION,
    )?;
    let value = reader.read_u64::<LittleEndian>()?;
    let rseed = read_sapling_rseed(&mut reader)?;

    let fvk = sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(
        &wallet_capability.unified_key_store,
    )
    .map_err(DecodeError::SaplingViewingKey)?;
    let address = fvk
        .fvk()
        .vk
        .to_payment_address(diversifier)
        .ok_or(DecodeError::SaplingDiversifier)?;
    Ok(address.create_note(sapling_crypto::value::NoteValue::from_raw(value), rseed))
}

// Reading a note also needs the corresponding address to read from.
fn read_sapling_rseed<R: Read>(mut reader: R) -> io::Result<sapling_crypto::Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    Ok(sapling_rseed(note_type, r_bytes)?)
}

const ORCHARD_NOTE_VERSION: u8 = 1;

fn read_orchard_note<R: Read>(
    mut reader: R,
    diversifier: orchard::keys::Diversifier,
    wallet_capability: &WalletCapability,
) -> io::Result<orchard::note::Note> {
    let _version = read_version(
        &mut reader,
        std::any::type_name::<orchard::note::Note>(),
        ORCHARD_NOTE_VERSION,
    )?;
    let value = reader.read_u64::<LittleEndian>()?;
    let mut nullifier_bytes = [0; 32];
    reader.read_exact(&mut nullifier_bytes)?;
    let rho_nullifier = orchard::note::Rho::from_bytes(&nullifier_bytes)
        .into_option()
        .ok_or(DecodeError::OrchardRho)?;

    let mut random_seed_bytes = [0; 32];
    reader.read_exact(&mut random_seed_bytes)?;
    let random_seed = orchard::note::RandomSeed::from_bytes(random_seed_bytes, &rho_nullifier)
        .into_option()
        .ok_or(DecodeError::OrchardRandomSeed)?;

    let fvk = orchard::keys::FullViewingKey::try_from(&wallet_capability.unified_key_store)
        .map_err(DecodeError::OrchardViewingKey)?;
    Ok(orchard::note::Note::from_parts(
        fvk.address(diversifier, orchard::keys::Scope::External),
        orchard::value::NoteValue::from_raw(value),
        rho_nullifier,
        random_seed,
        orchard::note::NoteVersion::V2,
    )
    .into_option()
    .ok_or(DecodeError::OrchardNote)?)
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, PartialEq)]
pub struct TransparentOutput {
    /// TODO: Add Doc Comment Here!
    pub address: String,
    /// TODO: Add Doc Comment Here!
    pub txid: TxId,
    /// TODO: Add Doc Comment Here!
    pub output_index: u64,
    /// TODO: Add Doc Comment Here!
    pub script: Vec<u8>,
    /// TODO: Add Doc Comment Here!
    pub value: u64,
    /// whether, where, and when it was spent
    spend: Option<(TxId, ConfirmationStatus)>,
    /// Output is from a coinbase transaction
    pub is_coinbase: bool,
}

impl TransparentOutput {
    /// TODO: Add Doc Comment Here!
    pub fn read<R: std::io::Read>(mut reader: R) -> std::io::Result<Self> {
        let version = reader.read_u64::<byteorder::LittleEndian>()?;

        let address_len = reader.read_u32::<byteorder::LittleEndian>()?;
        let address = strings::read_string_body(&mut reader, u64::from(address_len))?;
        let address = address
            .starts_with('t')
            .then_some(address)
            .ok_or(DecodeError::TransparentAddress)?;

        let mut transaction_id_bytes = [0; 32];
        reader.read_exact(&mut transaction_id_bytes)?;
        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let output_index = reader.read_u64::<byteorder::LittleEndian>()?;
        let value = reader.read_u64::<byteorder::LittleEndian>()?;
        let _height = reader.read_i32::<byteorder::LittleEndian>()?;

        let script = zcash_encoding::Vector::read(&mut reader, |r| {
            let mut byte = [0; 1];
            r.read_exact(&mut byte)?;
            Ok(byte[0])
        })?;

        let spent = zcash_encoding::Optional::read(&mut reader, |r| {
            let mut transaction_bytes = [0u8; 32];
            r.read_exact(&mut transaction_bytes)?;
            Ok(TxId::from_bytes(transaction_bytes))
        })?;

        let spent_at_height = if version <= 1 {
            None
        } else {
            zcash_encoding::Optional::read(&mut reader, |r| {
                r.read_i32::<byteorder::LittleEndian>()
            })?
        };

        let _pending_spent = if version == 3 {
            zcash_encoding::Optional::read(&mut reader, |r| {
                let mut transaction_bytes = [0u8; 32];
                r.read_exact(&mut transaction_bytes)?;

                let height = r.read_u32::<byteorder::LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_bytes), height))
            })?
        } else {
            None
        };

        let spent_tuple: Option<(TxId, u32)> = if let Some(txid) = spent {
            if let Some(height) = spent_at_height {
                Some((txid, height as u32))
            } else {
                Some((txid, 0))
            }
        } else {
            None
        };
        let spend =
            spent_tuple.map(|(txid, height)| (txid, ConfirmationStatus::Confirmed(height.into())));

        let is_coinbase = if version >= 5 {
            reader.read_u8()? != 0
        } else {
            false
        };

        Ok(TransparentOutput {
            address,
            txid: transaction_id,
            output_index,
            script,
            value,
            spend,
            is_coinbase,
        })
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone)]
#[allow(dead_code)]
pub struct SaplingNote {
    /// TODO: Add Doc Comment Here!
    pub diversifier: sapling_crypto::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub sapling_crypto_note: sapling_crypto::Note,
    // The position of this note's value commitment in the global commitment tree
    // We need to create a witness to it, to spend
    pub(crate) witnessed_position: Option<Position>,
    // The note's index in its containing transaction
    pub(crate) output_index: Option<u32>,
    /// TODO: Add Doc Comment Here!
    pub nullifier: Option<sapling_crypto::Nullifier>,
    /// whether, where, and when it was spent
    spend: Option<(TxId, ConfirmationStatus)>,
    /// TODO: Add Doc Comment Here!
    pub memo: Option<Memo>,
    /// DEPRECATED
    pub is_change: bool,
    /// If the spending key is available in the wallet (i.e., whether to keep witness up-to-date) Todo should this data point really be here?
    pub have_spending_key: bool,
}

impl
    ReadableWriteable<(
        &WalletCapability,
        Option<
            &mut Vec<(
                IncrementalWitness<sapling_crypto::Node, COMMITMENT_TREE_LEVELS>,
                BlockHeight,
            )>,
        >,
    )> for SaplingNote
{
    const VERSION: u8 = 5;

    fn read<R: Read>(
        mut reader: R,
        (wallet_capability, inc_wit_vec): (
            &WalletCapability,
            Option<
                &mut Vec<(
                    IncrementalWitness<sapling_crypto::Node, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
            >,
        ),
    ) -> io::Result<Self> {
        let external_version = Self::get_version(&mut reader)?;

        if external_version < 2 {
            reader.read_exact(&mut [0u8; SAPLING_DISCARDED_PREFIX_SIZE])?;
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = sapling_crypto::Diversifier(diversifier_bytes);

        let note = read_sapling_note(&mut reader, diversifier, wallet_capability)?;

        let witnessed_position = match external_version {
            5.. => Optional::read(&mut reader, <R>::read_u64::<LittleEndian>)?.map(Position::from),
            4 => Some(Position::from(reader.read_u64::<LittleEndian>()?)),
            ..4 => read_witness_cache(&mut reader, inc_wit_vec)?,
        };

        let read_nullifier = |r: &mut R| {
            let mut nullifier = [0u8; 32];
            r.read_exact(&mut nullifier)?;
            Ok(sapling_crypto::Nullifier::from_slice(&nullifier)
                .expect("a 32-byte array has the length of a sapling nullifier"))
        };

        let nullifier = match external_version {
            5.. => Optional::read(&mut reader, read_nullifier)?,
            ..5 => Some(read_nullifier(&mut reader)?),
        };

        let spend = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let status = if let 5.. = external_version {
                read_confirmation_status(r)
            } else {
                let height = r.read_u32::<LittleEndian>()?;
                Ok(ConfirmationStatus::Confirmed(BlockHeight::from_u32(height)))
            }?;
            Ok((TxId::from_bytes(transaction_id_bytes), status))
        })?;

        // Note that the spent field is now an enum, that contains what used to be
        // a separate 'pending_spent' field. As they're mutually exclusive states,
        // they are now stored in the same field.
        if external_version < 3 {
            let _pending_spent = {
                Optional::read(&mut reader, |r| {
                    let mut transaction_bytes = [0u8; 32];
                    r.read_exact(&mut transaction_bytes)?;

                    let height = r.read_u32::<LittleEndian>()?;
                    Ok((TxId::from_bytes(transaction_bytes), height))
                })?
            };
        }

        let memo = Optional::read(&mut reader, read_memo)?;

        let is_change: bool = reader.read_u8()? > 0;

        let have_spending_key = reader.read_u8()? > 0;

        let output_index = if external_version >= 4 {
            match reader.read_u32::<LittleEndian>()? {
                u32::MAX => None,
                otherwise => Some(otherwise),
            }
        } else {
            None
        };

        Ok(Self {
            diversifier,
            sapling_crypto_note: note,
            witnessed_position,
            nullifier,
            spend,
            memo,
            is_change,
            have_spending_key,
            output_index,
        })
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, PartialEq)]
pub struct OrchardNote {
    /// TODO: Add Doc Comment Here!
    pub diversifier: orchard::keys::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub orchard_crypto_note: orchard::note::Note,
    /// The position of this note's value commitment in the global commitment tree
    /// We need to create a witness to it, to spend
    pub witnessed_position: Option<Position>,
    /// The note's index in its containing transaction
    pub(crate) output_index: Option<u32>,
    pub(crate) nullifier: Option<orchard::note::Nullifier>,
    /// whether, where, and when it was spent
    spend: Option<(TxId, ConfirmationStatus)>,
    /// TODO: Add Doc Comment Here!
    pub memo: Option<Memo>,
    /// DEPRECATED
    pub is_change: bool,
    /// If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl
    ReadableWriteable<(
        &WalletCapability,
        Option<
            &mut Vec<(
                IncrementalWitness<MerkleHashOrchard, COMMITMENT_TREE_LEVELS>,
                BlockHeight,
            )>,
        >,
    )> for OrchardNote
{
    const VERSION: u8 = 5;

    fn read<R: Read>(
        mut reader: R,
        (wallet_capability, inc_wit_vec): (
            &WalletCapability,
            Option<
                &mut Vec<(
                    IncrementalWitness<MerkleHashOrchard, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
            >,
        ),
    ) -> io::Result<Self> {
        let external_version = Self::get_version(&mut reader)?;

        if external_version < 2 {
            reader.read_exact(&mut [0u8; ORCHARD_DISCARDED_PREFIX_SIZE])?;
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = orchard::keys::Diversifier::from_bytes(diversifier_bytes);

        let note = read_orchard_note(&mut reader, diversifier, wallet_capability)?;

        let witnessed_position = match external_version {
            5.. => Optional::read(&mut reader, <R>::read_u64::<LittleEndian>)?.map(Position::from),
            4 => Some(Position::from(reader.read_u64::<LittleEndian>()?)),
            ..4 => read_witness_cache(&mut reader, inc_wit_vec)?,
        };

        let read_nullifier = |r: &mut R| {
            let mut nullifier = [0u8; 32];
            r.read_exact(&mut nullifier)?;
            Ok(orchard_nullifier(&nullifier)?)
        };

        let nullifier = match external_version {
            5.. => Optional::read(&mut reader, read_nullifier)?,
            ..5 => Some(read_nullifier(&mut reader)?),
        };

        let spend = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let status = if let 5.. = external_version {
                read_confirmation_status(r)
            } else {
                let height = r.read_u32::<LittleEndian>()?;
                Ok(ConfirmationStatus::Confirmed(BlockHeight::from_u32(height)))
            }?;
            Ok((TxId::from_bytes(transaction_id_bytes), status))
        })?;

        // Note that the spent field is now an enum, that contains what used to be
        // a separate 'pending_spent' field. As they're mutually exclusive states,
        // they are now stored in the same field.
        if external_version < 3 {
            let _pending_spent = {
                Optional::read(&mut reader, |r| {
                    let mut transaction_bytes = [0u8; 32];
                    r.read_exact(&mut transaction_bytes)?;

                    let height = r.read_u32::<LittleEndian>()?;
                    Ok((TxId::from_bytes(transaction_bytes), height))
                })?
            };
        }

        let memo = Optional::read(&mut reader, read_memo)?;

        let is_change: bool = reader.read_u8()? > 0;

        let have_spending_key = reader.read_u8()? > 0;

        let output_index = if external_version >= 4 {
            match reader.read_u32::<LittleEndian>()? {
                u32::MAX => None,
                otherwise => Some(otherwise),
            }
        } else {
            None
        };

        Ok(Self {
            diversifier,
            orchard_crypto_note: note,
            witnessed_position,
            nullifier,
            spend,
            memo,
            is_change,
            have_spending_key,
            output_index,
        })
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

/// Only for `TransactionRecords` *from* "this" capability
#[derive(Clone)]
#[allow(dead_code)]
pub struct OutgoingTxData {
    /// TODO: Add Doc Comment Here!
    pub recipient_address: String,
    /// Amount to this receiver
    pub value: u64,
    /// Note to the receiver, why not an option?
    pub memo: Memo,
    /// What if it wasn't provided?  How does this relate to
    /// `recipient_address`?
    pub recipient_ua: Option<String>,
    /// This output's index in its containing transaction
    pub output_index: Option<u64>,
}

impl OutgoingTxData {
    /// Before version 0, `OutgoingTxData` didn't have a version field
    pub fn read_old<R: Read>(mut reader: R) -> io::Result<Self> {
        let address = strings::read_string(&mut reader)?;

        let value = reader.read_u64::<LittleEndian>()?;

        let memo = read_memo(&mut reader)?;

        Ok(OutgoingTxData {
            recipient_address: address,
            value,
            memo,
            recipient_ua: None,
            output_index: None,
        })
    }

    /// Read an `OutgoingTxData` from its serialized
    /// representation
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let _external_version = CompactSize::read(&mut reader)?;
        let address = strings::read_string(&mut reader)?;

        let value = reader.read_u64::<LittleEndian>()?;

        let memo = read_memo(&mut reader)?;
        let output_index = Optional::read(&mut reader, CompactSize::read)?;

        Ok(OutgoingTxData {
            recipient_address: address,
            value,
            memo,
            recipient_ua: None,
            output_index,
        })
    }
}

/// TODO: Add Doc Comment Here!
pub const COMMITMENT_TREE_LEVELS: u8 = 32;
/// TODO: Add Doc Comment Here!
pub const MAX_SHARD_LEVEL: u8 = 16;
/// Witness-tree checkpoint retention depth, derived from the
/// repository's single max-reorg truth (which in turn mirrors zebra's
/// finalization boundary, see the source constant's docs).
pub const MAX_REORG: usize = zl_common::wallet::MAX_REORG_ALLOWANCE as usize;

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub struct WitnessTrees {
    /// TODO: Add Doc Comment Here!
    pub witness_tree_sapling: ShardTree<SapStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
    /// TODO: Add Doc Comment Here!
    pub witness_tree_orchard: ShardTree<OrchStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
}

impl WitnessTrees {
    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let _serialized_version = reader.read_u8()?;
        let witness_tree_sapling = read_shardtree(&mut reader)?;
        let witness_tree_orchard = read_shardtree(reader)?;
        Ok(Self {
            witness_tree_sapling,
            witness_tree_orchard,
        })
    }
}

impl Default for WitnessTrees {
    fn default() -> WitnessTrees {
        Self {
            witness_tree_sapling: shardtree::ShardTree::new(MemoryShardStore::empty(), MAX_REORG),
            witness_tree_orchard: shardtree::ShardTree::new(MemoryShardStore::empty(), MAX_REORG),
        }
    }
}

fn read_shardtree<
    H: Hashable + Clone + HashSer + Eq,
    C: Ord + std::fmt::Debug + Copy + From<u32>,
    R: Read,
>(
    mut reader: R,
) -> io::Result<shardtree::ShardTree<MemoryShardStore<H, C>, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>>
{
    let shards = Vector::read(&mut reader, |r| {
        let level = Level::from(r.read_u8()?);
        let index = r.read_u64::<LittleEndian>()?;
        let root_addr = Address::from_parts(level, index);
        let shard = read_shard(r)?;

        Ok(LocatedPrunableTree::from_parts(root_addr, shard).map_err(DecodeError::ShardRoot)?)
    })?;
    let mut store = MemoryShardStore::empty();
    for shard in shards {
        store.put_shard(shard).expect("Infallible");
    }
    let checkpoints = Vector::read(&mut reader, |r| {
        let checkpoint_id = C::from(r.read_u32::<LittleEndian>()?);
        let tree_state = match r.read_u8()? {
            0 => shardtree::store::TreeState::Empty,
            1 => shardtree::store::TreeState::AtPosition(Position::from(
                r.read_u64::<LittleEndian>()?,
            )),
            otherwise => return Err(DecodeError::TreeState(otherwise).into()),
        };
        let marks_removed = Vector::read(r, |r| r.read_u64::<LittleEndian>().map(Position::from))?;
        Ok((
            checkpoint_id,
            Checkpoint::from_parts(tree_state, marks_removed.into_iter().collect()),
        ))
    })?;
    for (checkpoint_id, checkpoint) in checkpoints {
        store
            .add_checkpoint(checkpoint_id, checkpoint)
            .expect("Infallible");
    }
    store.put_cap(read_shard(reader)?).expect("Infallible");
    Ok(shardtree::ShardTree::new(store, MAX_REORG))
}

pub(crate) type SapStore = MemoryShardStore<sapling_crypto::Node, BlockHeight>;
pub(crate) type OrchStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;

const CONFIRMATION_STATUS_VERSION: u8 = 0;

fn read_confirmation_status<R: Read>(mut reader: R) -> io::Result<ConfirmationStatus> {
    let _external_version = read_version(
        &mut reader,
        std::any::type_name::<ConfirmationStatus>(),
        CONFIRMATION_STATUS_VERSION,
    )?;
    let status = reader.read_u8()?;
    let height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
    match status {
        0 => Ok(ConfirmationStatus::Calculated(height)),
        1 => Ok(ConfirmationStatus::Transmitted(height)),
        2 => Ok(ConfirmationStatus::Mempool(height)),
        3 => Ok(ConfirmationStatus::Confirmed(height)),
        _ => Err(DecodeError::ConfirmationStatus.into()),
    }
}

/// TODO: Add Doc Comment Here!
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoDownloadOption {
    /// TODO: Add Doc Comment Here!
    NoMemos,
    /// TODO: Add Doc Comment Here!
    WalletMemos,
    /// TODO: Add Doc Comment Here!
    AllMemos,
}

/// TODO: Add Doc Comment Here!
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct WalletOptions {
    pub(crate) download_memos: MemoDownloadOption,
    /// TODO: Add Doc Comment Here!
    pub transaction_size_filter: Option<u32>,
}

/// TODO: Add Doc Comment Here!
pub const MAX_TRANSACTION_SIZE_DEFAULT: u32 = 500;

impl Default for WalletOptions {
    fn default() -> Self {
        WalletOptions {
            download_memos: MemoDownloadOption::WalletMemos,
            transaction_size_filter: Some(MAX_TRANSACTION_SIZE_DEFAULT),
        }
    }
}

impl WalletOptions {
    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let external_version = reader.read_u64::<LittleEndian>()?;

        let download_memos = match reader.read_u8()? {
            0 => MemoDownloadOption::NoMemos,
            1 => MemoDownloadOption::WalletMemos,
            2 => MemoDownloadOption::AllMemos,
            v => return Err(DecodeError::MemoDownloadOption(v).into()),
        };

        let transaction_size_filter = if external_version > 1 {
            Optional::read(reader, |mut r| r.read_u32::<LittleEndian>())?
        } else {
            Some(500)
        };

        Ok(Self {
            download_memos,
            transaction_size_filter,
        })
    }
}

/// Struct that tracks the latest and historical price of ZEC in the wallet
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct WalletZecPriceInfo {
    /// Latest price of ZEC and when it was fetched
    pub zec_price: Option<(u64, f64)>,

    /// Wallet's currency. All the prices are in this currency
    pub currency: String,

    /// When the last time historical prices were fetched
    pub last_historical_prices_fetched_at: Option<u64>,

    /// Historical prices retry count
    pub historical_prices_retry_count: u64,
}

impl Default for WalletZecPriceInfo {
    fn default() -> Self {
        Self {
            zec_price: None,
            currency: "USD".to_string(), // Only USD is supported right now.
            last_historical_prices_fetched_at: None,
            historical_prices_retry_count: 0,
        }
    }
}

impl WalletZecPriceInfo {
    /// TODO: Add Doc Comment Here!
    pub fn serialized_version() -> u64 {
        20
    }

    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(DecodeError::Version("ZecPriceInfo").into());
        }

        // The "current" zec price is not persisted, since it is almost certainly outdated
        let zec_price = None;

        // Currency is only USD for now
        let currency = "USD".to_string();

        let last_historical_prices_fetched_at = Optional::read(
            &mut reader,
            byteorder::ReadBytesExt::read_u64::<LittleEndian>,
        )?;
        let historical_prices_retry_count = reader.read_u64::<LittleEndian>()?;

        Ok(Self {
            zec_price,
            currency,
            last_historical_prices_fetched_at,
            historical_prices_retry_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSEED_BEFORE_ZIP212: u8 = 1;
    const RSEED_UNKNOWN: u8 = 3;
    const CONFIRMED_STATUS: u8 = 3;
    const CONFIRMED_HEIGHT: u32 = 7;

    fn confirmation_status_record(version: u8) -> Vec<u8> {
        let mut record = vec![version, CONFIRMED_STATUS];
        record.extend(CONFIRMED_HEIGHT.to_le_bytes());
        record
    }

    #[test]
    fn a_confirmation_status_of_the_written_version_reads() {
        let record = confirmation_status_record(CONFIRMATION_STATUS_VERSION);
        assert_eq!(
            read_confirmation_status(record.as_slice()).ok(),
            Some(ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                CONFIRMED_HEIGHT
            )))
        );
    }

    #[test]
    fn a_confirmation_status_of_a_later_version_is_refused() {
        let record = confirmation_status_record(CONFIRMATION_STATUS_VERSION + 1);
        assert_eq!(
            read_confirmation_status(record.as_slice())
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn a_non_canonical_orchard_nullifier_is_its_own_decode_error() {
        assert!(matches!(
            orchard_nullifier(&[u8::MAX; _]),
            Err(DecodeError::OrchardNullifier)
        ));
    }

    #[test]
    fn a_canonical_orchard_nullifier_round_trips() {
        let canonical = [u8::MIN; _];
        assert_eq!(
            orchard_nullifier(&canonical)
                .ok()
                .map(|nullifier| nullifier.to_bytes()),
            Some(canonical)
        );
    }

    #[test]
    fn a_decode_error_reaches_the_caller_as_invalid_data() {
        assert_eq!(
            io::Error::from(DecodeError::OrchardNullifier).kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn each_rseed_decode_error_is_its_own_variant() {
        assert!(matches!(
            sapling_rseed(RSEED_BEFORE_ZIP212, [u8::MAX; _]),
            Err(DecodeError::SaplingRseedScalar)
        ));
        assert!(matches!(
            sapling_rseed(RSEED_UNKNOWN, [u8::MIN; _]),
            Err(DecodeError::SaplingRseedType(found)) if found == RSEED_UNKNOWN
        ));
    }

    #[test]
    fn memo_bytes_longer_than_a_memo_are_a_memo_decode_error() {
        assert!(matches!(
            memo(&[u8::MIN; MEMO_SIZE + 1]),
            Err(DecodeError::Memo(_))
        ));
    }

    #[test]
    fn a_memo_error_prints_its_own_layer_and_keeps_the_cause_as_its_source() {
        let error = memo(&[u8::MIN; MEMO_SIZE + 1]).err();
        assert_eq!(
            error.as_ref().map(ToString::to_string).as_deref(),
            Some("Couldn't create memo")
        );
        assert!(
            error
                .as_ref()
                .and_then(std::error::Error::source)
                .is_some_and(|source| source.is::<zcash_protocol::memo::Error>())
        );
    }
}

#[cfg(test)]
mod corrupt_input {
    use zcash_primitives::merkle_tree::write_incremental_witness;

    use super::*;
    use zl_common::{chain::ChainType, keys::UnifiedKeyStore};

    const TRANSPARENT_OUTPUT_VERSION: u64 = 5;
    const NOTE_WITH_DISCARDED_PREFIX: u8 = 1;
    const NOTE_WITH_WITNESS_CACHE: u8 = 3;
    const NOTE_CURRENT: u8 = 5;
    const INNER_NOTE_VERSION: u8 = 1;
    const RSEED_AFTER_ZIP212: u8 = 2;
    const NOTE_VALUE: u64 = 1;
    const ONE_RECORD: u8 = 1;
    const TWO_WITNESSES: u8 = 2;
    const TOP_HEIGHT_BELOW_WITNESS_COUNT: u64 = 0;

    fn kind<T>(read: io::Result<T>) -> Option<io::ErrorKind> {
        read.err().map(|error| error.kind())
    }

    fn decode_error<T>(read: io::Result<T>) -> Option<DecodeError> {
        let error = read.err()?.into_inner()?.downcast::<DecodeError>().ok()?;
        Some(*error)
    }

    fn reports_a_key_error_as_its_source<T>(read: io::Result<T>) -> bool {
        read.err()
            .as_ref()
            .and_then(std::error::Error::source)
            .is_some_and(|source| source.is::<KeyError>())
    }

    fn transparent_output(address_len: i32, address: &[u8]) -> Vec<u8> {
        [
            &TRANSPARENT_OUTPUT_VERSION.to_le_bytes()[..],
            &address_len.to_le_bytes(),
            address,
        ]
        .concat()
    }

    fn sapling_note(version: u8, diversifier: sapling_crypto::Diversifier) -> Vec<u8> {
        [
            &[version][..],
            &diversifier.0,
            &[INNER_NOTE_VERSION],
            &NOTE_VALUE.to_le_bytes(),
            &[RSEED_AFTER_ZIP212],
            &[u8::MIN; 32],
        ]
        .concat()
    }

    fn orchard_note() -> Vec<u8> {
        [
            &[NOTE_CURRENT][..],
            &[u8::MIN; 11],
            &[INNER_NOTE_VERSION],
            &NOTE_VALUE.to_le_bytes(),
            &[u8::MIN; 32],
            &[u8::MIN; 32],
        ]
        .concat()
    }

    fn diversifier(on_curve: bool) -> io::Result<sapling_crypto::Diversifier> {
        (u8::MIN..=u8::MAX)
            .map(|byte| sapling_crypto::Diversifier([byte; 11]))
            .find(|diversifier| diversifier.g_d().is_some() == on_curve)
            .ok_or_else(|| io::Error::other("no single-byte diversifier has the wanted validity"))
    }

    fn spending_capability() -> io::Result<WalletCapability> {
        let mut capability = WalletCapability::default();
        capability.unified_key_store = UnifiedKeyStore::new_from_seed(
            ChainType::Mainnet,
            &[u8::MIN; 64],
            zip32::AccountId::ZERO,
        )
        .map_err(io::Error::other)?;
        Ok(capability)
    }

    #[test]
    fn a_transparent_output_with_an_empty_address_is_invalid_data() {
        assert_eq!(
            kind(TransparentOutput::read(
                transparent_output(0, b"").as_slice()
            )),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn a_transparent_output_with_a_shielded_address_is_invalid_data() {
        assert_eq!(
            kind(TransparentOutput::read(
                transparent_output(1, b"z").as_slice()
            )),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn a_transparent_output_with_a_negative_address_length_is_invalid_data() {
        assert_eq!(
            kind(TransparentOutput::read(
                transparent_output(-1, b"").as_slice()
            )),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn outgoing_data_with_an_unbounded_address_length_is_invalid_data() {
        let unbounded = u64::MAX.to_le_bytes();
        assert_eq!(
            kind(OutgoingTxData::read_old(unbounded.as_slice())),
            Some(io::ErrorKind::InvalidData)
        );
        let versioned = [&[u8::MIN][..], &unbounded].concat();
        assert_eq!(
            kind(OutgoingTxData::read(versioned.as_slice())),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn a_note_truncated_in_its_discarded_prefix_is_an_unexpected_eof() {
        let truncated = [NOTE_WITH_DISCARDED_PREFIX];
        let capability = WalletCapability::default();
        assert_eq!(
            kind(SaplingNote::read(truncated.as_slice(), (&capability, None))),
            Some(io::ErrorKind::UnexpectedEof)
        );
        assert_eq!(
            kind(OrchardNote::read(truncated.as_slice(), (&capability, None))),
            Some(io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn a_sapling_note_in_a_wallet_without_a_sapling_key_is_invalid_data() -> io::Result<()> {
        let note = sapling_note(NOTE_CURRENT, diversifier(true)?);
        assert_eq!(
            kind(SaplingNote::read(
                note.as_slice(),
                (&WalletCapability::default(), None)
            )),
            Some(io::ErrorKind::InvalidData)
        );
        Ok(())
    }

    #[test]
    fn a_missing_viewing_key_reaches_the_caller_with_its_key_error_as_the_source() -> io::Result<()>
    {
        let capability = WalletCapability::default();
        let sapling = sapling_note(NOTE_CURRENT, diversifier(true)?);
        assert!(reports_a_key_error_as_its_source(SaplingNote::read(
            sapling.as_slice(),
            (&capability, None)
        )));
        assert!(reports_a_key_error_as_its_source(OrchardNote::read(
            orchard_note().as_slice(),
            (&capability, None)
        )));
        Ok(())
    }

    #[test]
    fn a_diversifier_off_the_curve_is_the_diversifier_decode_error() -> io::Result<()> {
        let note = sapling_note(NOTE_CURRENT, diversifier(false)?);
        assert!(matches!(
            decode_error(SaplingNote::read(
                note.as_slice(),
                (&spending_capability()?, None)
            )),
            Some(DecodeError::SaplingDiversifier)
        ));
        Ok(())
    }

    #[test]
    fn a_witness_cache_taller_than_its_top_height_is_invalid_data() -> io::Result<()> {
        let mut tree = sapling_crypto::CommitmentTree::empty();
        tree.append(sapling_crypto::Node::empty_leaf())
            .map_err(|()| io::Error::other("an empty tree refused its first leaf"))?;
        let witness = IncrementalWitness::<_, COMMITMENT_TREE_LEVELS>::from_tree(tree)
            .ok_or_else(|| io::Error::other("a one-leaf tree gave no witness"))?;
        let mut note = sapling_note(NOTE_WITH_WITNESS_CACHE, diversifier(true)?);
        note.push(TWO_WITNESSES);
        write_incremental_witness(&witness, &mut note)?;
        write_incremental_witness(&witness, &mut note)?;
        note.extend(TOP_HEIGHT_BELOW_WITNESS_COUNT.to_le_bytes());
        assert_eq!(
            kind(SaplingNote::read(
                note.as_slice(),
                (&spending_capability()?, None)
            )),
            Some(io::ErrorKind::InvalidData)
        );
        Ok(())
    }

    #[test]
    fn a_truncated_transaction_record_propagates_its_error() {
        let truncated = [&[ONE_RECORD][..], &[u8::MIN; 32]].concat();
        assert_eq!(
            kind(TxMap::read_old(
                truncated.as_slice(),
                &WalletCapability::default()
            )),
            Some(io::ErrorKind::UnexpectedEof)
        );
    }
}
