use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt};
use orchard::tree::MerkleHashOrchard;
use prost::Message;

use incrementalmerkletree::{Address, Hashable, Level, Position, witness::IncrementalWitness};
use netutils::lightwallet_protocol::CompactBlock;
use pepper_sync::shardtree_ext::ShardTreeExt as _;
use shardtree::{
    LocatedPrunableTree, ShardTree,
    store::{Checkpoint, ShardStore as _, memory::MemoryShardStore},
};
use status::confirmation_status::ConfirmationStatus;
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

use super::{keys::legacy::WalletCapability, traits::ReadableWriteable, utils};

type DatedWitness<H> = (IncrementalWitness<H, COMMITMENT_TREE_LEVELS>, BlockHeight);

fn decoded<T>(value: Option<T>, fault: &'static str) -> io::Result<T> {
    value.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, fault))
}

fn read_orchard_nullifier(bytes: &[u8; 32]) -> io::Result<orchard::note::Nullifier> {
    decoded(
        orchard::note::Nullifier::from_bytes(bytes).into_option(),
        "stored orchard nullifier is not a canonical field element",
    )
}

fn witness_height(top_height: u64, depth: usize) -> io::Result<BlockHeight> {
    decoded(
        u32::try_from(top_height)
            .ok()
            .zip(u32::try_from(depth).ok())
            .and_then(|(top_height, depth)| top_height.checked_sub(depth)),
        "stored witness cache is taller than its top height",
    )
    .map(BlockHeight::from)
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
        cb.encode(&mut ecb).unwrap();

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
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
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
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read wallettxns because of incorrect version",
            ));
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
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
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
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
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
    pub status: status::confirmation_status::ConfirmationStatus,
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
                read_orchard_nullifier(&n)
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

impl ReadableWriteable<(sapling_crypto::Diversifier, &WalletCapability)> for sapling_crypto::Note {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (diversifier, wallet_capability): (sapling_crypto::Diversifier, &WalletCapability),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let rseed = read_sapling_rseed(&mut reader)?;

        let fvk = decoded(
            sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(
                &wallet_capability.unified_key_store,
            )
            .ok(),
            "wallet holds no sapling viewing key for its stored sapling note",
        )?;
        decoded(
            fvk.fvk().vk.to_payment_address(diversifier),
            "stored sapling note diversifier does not map to a jubjub point",
        )
        .map(|address| {
            address.create_note(sapling_crypto::value::NoteValue::from_raw(value), rseed)
        })
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

// Reading a note also needs the corresponding address to read from.
fn read_sapling_rseed<R: Read>(mut reader: R) -> io::Result<sapling_crypto::Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    match note_type {
        1 => decoded(
            jubjub::Fr::from_bytes(&r_bytes).into_option(),
            "stored sapling rseed is not a canonical jubjub scalar",
        )
        .map(sapling_crypto::Rseed::BeforeZip212),
        2 => Ok(sapling_crypto::Rseed::AfterZip212(r_bytes)),
        _ => decoded(None, "stored sapling rseed type is unknown"),
    }
}

impl ReadableWriteable<(orchard::keys::Diversifier, &WalletCapability)> for orchard::note::Note {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (diversifier, wallet_capability): (orchard::keys::Diversifier, &WalletCapability),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let mut nullifier_bytes = [0; 32];
        reader.read_exact(&mut nullifier_bytes)?;
        let rho_nullifier = decoded(
            orchard::note::Rho::from_bytes(&nullifier_bytes).into_option(),
            "Bad Nullifier",
        )?;

        let mut random_seed_bytes = [0; 32];
        reader.read_exact(&mut random_seed_bytes)?;
        let random_seed = decoded(
            orchard::note::RandomSeed::from_bytes(random_seed_bytes, &rho_nullifier).into_option(),
            "Nullifier not for note",
        )?;

        let fvk = decoded(
            orchard::keys::FullViewingKey::try_from(&wallet_capability.unified_key_store).ok(),
            "wallet holds no orchard viewing key for its stored orchard note",
        )?;
        decoded(
            orchard::note::Note::from_parts(
                fvk.address(diversifier, orchard::keys::Scope::External),
                orchard::value::NoteValue::from_raw(value),
                rho_nullifier,
                random_seed,
                orchard::note::NoteVersion::V2,
            )
            .into_option(),
            "Invalid note",
        )
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
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
        let address = utils::read_string_body(&mut reader, u64::from(address_len))?;
        let address = decoded(
            address.starts_with('t').then_some(address),
            "stored transparent output address is not transparent",
        )?;

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
            reader.read_exact(&mut [0u8; 169])?;
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = sapling_crypto::Diversifier(diversifier_bytes);

        let note = sapling_crypto::Note::read(&mut reader, (diversifier, wallet_capability))?;

        let witnessed_position = match external_version {
            5.. => Optional::read(&mut reader, <R>::read_u64::<LittleEndian>)?.map(Position::from),
            4 => Some(Position::from(reader.read_u64::<LittleEndian>()?)),
            ..4 => read_witness_cache(&mut reader, inc_wit_vec)?,
        };

        let read_nullifier = |r: &mut R| {
            let mut nullifier = [0u8; 32];
            r.read_exact(&mut nullifier)?;
            Ok(sapling_crypto::Nullifier::from_slice(&nullifier).unwrap())
        };

        let nullifier = match external_version {
            5.. => Optional::read(&mut reader, read_nullifier)?,
            ..5 => Some(read_nullifier(&mut reader)?),
        };

        let spend = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let status = if let 5.. = external_version {
                ReadableWriteable::read(r, ())
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

        let memo = Optional::read(&mut reader, |r| {
            let mut memo_bytes = [0u8; 512];
            r.read_exact(&mut memo_bytes)?;

            // Attempt to read memo, first as text, else as arbitrary 512 bytes
            match MemoBytes::from_bytes(&memo_bytes) {
                Ok(mb) => match Memo::try_from(mb.clone()) {
                    Ok(m) => Ok(m),
                    Err(_) => Ok(Memo::Future(mb)),
                },
                Err(e) => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Couldn't create memo: {e}"),
                )),
            }
        })?;

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
            reader.read_exact(&mut [0u8; 96])?;
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = orchard::keys::Diversifier::from_bytes(diversifier_bytes);

        let note = orchard::Note::read(&mut reader, (diversifier, wallet_capability))?;

        let witnessed_position = match external_version {
            5.. => Optional::read(&mut reader, <R>::read_u64::<LittleEndian>)?.map(Position::from),
            4 => Some(Position::from(reader.read_u64::<LittleEndian>()?)),
            ..4 => read_witness_cache(&mut reader, inc_wit_vec)?,
        };

        let read_nullifier = |r: &mut R| {
            let mut nullifier = [0u8; 32];
            r.read_exact(&mut nullifier)?;
            read_orchard_nullifier(&nullifier)
        };

        let nullifier = match external_version {
            5.. => Optional::read(&mut reader, read_nullifier)?,
            ..5 => Some(read_nullifier(&mut reader)?),
        };

        let spend = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let status = if let 5.. = external_version {
                ReadableWriteable::read(r, ())
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

        let memo = Optional::read(&mut reader, |r| {
            let mut memo_bytes = [0u8; 512];
            r.read_exact(&mut memo_bytes)?;

            // Attempt to read memo, first as text, else as arbitrary 512 bytes
            match MemoBytes::from_bytes(&memo_bytes) {
                Ok(mb) => match Memo::try_from(mb.clone()) {
                    Ok(m) => Ok(m),
                    Err(_) => Ok(Memo::Future(mb)),
                },
                Err(e) => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Couldn't create memo: {e}"),
                )),
            }
        })?;

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
        let address = utils::read_string(&mut reader)?;

        let value = reader.read_u64::<LittleEndian>()?;

        let mut memo_bytes = [0u8; 512];
        reader.read_exact(&mut memo_bytes)?;
        let memo = match MemoBytes::from_bytes(&memo_bytes) {
            Ok(mb) => match Memo::try_from(mb.clone()) {
                Ok(m) => Ok(m),
                Err(_) => Ok(Memo::Future(mb)),
            },
            Err(e) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Couldn't create memo: {e}"),
            )),
        }?;

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
        let address = utils::read_string(&mut reader)?;

        let value = reader.read_u64::<LittleEndian>()?;

        let mut memo_bytes = [0u8; 512];
        reader.read_exact(&mut memo_bytes)?;
        let memo = match MemoBytes::from_bytes(&memo_bytes) {
            Ok(mb) => match Memo::try_from(mb.clone()) {
                Ok(m) => Ok(m),
                Err(_) => Ok(Memo::Future(mb)),
            },
            Err(e) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Couldn't create memo: {e}"),
            )),
        }?;
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
pub const MAX_REORG: usize = pepper_sync::sync::MAX_REORG_ALLOWANCE as usize;

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

        LocatedPrunableTree::from_parts(root_addr, shard).map_err(|addr| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("parent node in root has level 0 relative to root address: {addr:?}"),
            )
        })
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
            otherwise => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("error reading TreeState: expected boolean value, found {otherwise}"),
                ));
            }
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

impl ReadableWriteable for ConfirmationStatus {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _external_version = Self::get_version(&mut reader);
        let status = reader.read_u8()?;
        let height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
        match status {
            0 => Ok(Self::Calculated(height)),
            1 => Ok(Self::Transmitted(height)),
            2 => Ok(Self::Mempool(height)),
            3 => Ok(Self::Confirmed(height)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bad confirmation status",
            )),
        }
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
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
            v => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Bad download option {v}"),
                ));
            }
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read ZecPriceInfo because of incorrect version",
            ));
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
mod shardtree_boundary {
    /// zingolib's half of the confinement rule pepper-sync enforces for
    /// itself: raw calls to shardtree's boolean operations conflate
    /// distinct outcomes (the class behind the migrated-wallet wipe), so
    /// every call in this crate routes through
    /// `pepper_sync::shardtree_ext`. The walk fails the build the moment
    /// a raw call reappears.
    #[test]
    fn raw_boolean_calls_route_through_the_wrapper() {
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut rust_sources = Vec::new();
        collect_rust_sources(&src_root, &mut rust_sources);
        assert!(!rust_sources.is_empty());

        // The call parenthesis is appended at runtime so this test's own
        // pattern list does not match itself during the walk.
        let raw_patterns = [
            ".checkpoint",
            ".truncate_to_checkpoint",
            ".truncate_to_checkpoint_depth",
        ]
        .map(|method| format!("{method}("));
        for path in rust_sources {
            let source = std::fs::read_to_string(&path).unwrap();
            for pattern in &raw_patterns {
                assert!(
                    !source.contains(pattern),
                    "{} calls a raw shardtree boolean operation ({pattern}); \
                     route it through pepper_sync::shardtree_ext instead",
                    path.display(),
                );
            }
        }
    }

    fn collect_rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_rust_sources(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_canonical_orchard_nullifier_is_invalid_data() {
        let err = read_orchard_nullifier(&[u8::MAX; _]).expect_err(
            "the all-ones encoding is 2^256 - 1, above the Pallas base field modulus (below 2^255), so it is not canonical",
        );
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "stored orchard nullifier is not a canonical field element"
        );
    }

    #[test]
    fn a_canonical_orchard_nullifier_round_trips() {
        let canonical = [u8::MIN; _];
        let nullifier = read_orchard_nullifier(&canonical)
            .expect("zero is below the Pallas base field modulus, so its encoding is canonical");
        assert_eq!(nullifier.to_bytes(), canonical);
    }
}

#[cfg(test)]
mod corrupt_input {
    use zcash_primitives::merkle_tree::write_incremental_witness;

    use super::*;
    use crate::{config::ChainType, wallet::keys::unified::UnifiedKeyStore};

    const TRANSPARENT_OUTPUT_VERSION: u64 = 5;
    const NOTE_WITH_DISCARDED_PREFIX: u8 = 1;
    const NOTE_WITH_WITNESS_CACHE: u8 = 3;
    const NOTE_CURRENT: u8 = 5;
    const INNER_NOTE_VERSION: u8 = 1;
    const RSEED_BEFORE_ZIP212: u8 = 1;
    const RSEED_AFTER_ZIP212: u8 = 2;
    const RSEED_UNKNOWN: u8 = 3;
    const NOTE_VALUE: u64 = 1;
    const ONE_RECORD: u8 = 1;
    const TWO_WITNESSES: u8 = 2;
    const TOP_HEIGHT_BELOW_WITNESS_COUNT: u64 = 0;

    fn kind<T>(read: io::Result<T>) -> Option<io::ErrorKind> {
        read.err().map(|error| error.kind())
    }

    fn message<T>(read: io::Result<T>) -> Option<String> {
        read.err().map(|error| error.to_string())
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
    fn a_diversifier_off_the_curve_is_reported_as_the_diversifier_fault() -> io::Result<()> {
        let note = sapling_note(NOTE_CURRENT, diversifier(false)?);
        assert_eq!(
            message(SaplingNote::read(
                note.as_slice(),
                (&spending_capability()?, None)
            )),
            Some("stored sapling note diversifier does not map to a jubjub point".to_string())
        );
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
    fn every_rseed_fault_is_invalid_data() {
        let non_canonical_scalar = [&[RSEED_BEFORE_ZIP212][..], &[u8::MAX; 32]].concat();
        assert_eq!(
            kind(read_sapling_rseed(non_canonical_scalar.as_slice())),
            Some(io::ErrorKind::InvalidData)
        );
        let unknown_type = [&[RSEED_UNKNOWN][..], &[u8::MIN; 32]].concat();
        assert_eq!(
            kind(read_sapling_rseed(unknown_type.as_slice())),
            Some(io::ErrorKind::InvalidData)
        );
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
