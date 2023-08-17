use crate::compact_formats::CompactBlock;
use crate::wallet::traits::ReceivedNoteAndMetadata;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::frontier::{CommitmentTree, NonEmptyFrontier};
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::{Address, Hashable, Level, Position};
use orchard::note_encryption::OrchardDomain;
use orchard::tree::MerkleHashOrchard;
use prost::Message;
use shardtree::memory::MemoryShardStore;
use shardtree::{Checkpoint, LocatedPrunableTree, ShardStore, ShardTree};
use std::convert::TryFrom;
use std::io::{self, Read, Write};
use std::usize;
use zcash_client_sqlite::serialization::{read_shard, write_shard};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::merkle_tree::{read_commitment_tree, write_commitment_tree, HashSer};
use zcash_primitives::sapling::note_encryption::SaplingDomain;
use zcash_primitives::sapling::{self, Node};
use zcash_primitives::{
    memo::Memo,
    transaction::{components::OutPoint, TxId},
};
use zingoconfig::{ChainType, MAX_REORG};

use super::keys::unified::WalletCapability;
use super::traits::{self, DomainWalletExt, ReadableWriteable, ToBytes};

pub const COMMITMENT_TREE_LEVELS: u8 = 32;
pub const MAX_SHARD_LEVEL: u8 = 16;

/// This type is motivated by the IPC architecture where (currently) channels traffic in
/// `(TxId, WalletNullifier, BlockHeight, Option<u32>)`.  This enum permits a single channel
/// type to handle nullifiers from different domains.
/// <https://github.com/zingolabs/zingolib/issues/64>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolNullifier {
    Sapling(zcash_primitives::sapling::Nullifier),
    Orchard(orchard::note::Nullifier),
}

type SapStore = MemoryShardStore<Node, BlockHeight>;
type OrchStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;
#[derive(Debug)]
pub struct WitnessTrees {
    pub witness_tree_sapling: ShardTree<SapStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
    pub witness_tree_orchard: ShardTree<OrchStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
}

fn write_shards<W, H, C>(mut writer: W, store: &MemoryShardStore<H, C>) -> io::Result<()>
where
    H: Hashable + Clone + Eq + HashSer,
    C: Ord + std::fmt::Debug + Copy,
    W: Write,
{
    let roots = store.get_shard_roots().expect("Infallible");
    Vector::write(&mut writer, &roots, |w, root| {
        w.write_u8(root.level().into())?;
        w.write_u64::<LittleEndian>(root.index())?;
        let shard = store
            .get_shard(*root)
            .expect("Infallible")
            .expect("cannot find root that shard store claims to have");
        write_shard(w, shard.root())?; // s.root returns &Tree
        Ok(())
    })?;
    Ok(())
}
fn write_checkpoints<W, Cid>(mut writer: W, checkpoints: &[(Cid, Checkpoint)]) -> io::Result<()>
where
    W: Write,
    Cid: Ord + std::fmt::Debug + Copy,
    u32: From<Cid>,
{
    Vector::write(
        &mut writer,
        checkpoints,
        |mut w, (checkpoint_id, checkpoint)| {
            w.write_u32::<LittleEndian>(u32::from(*checkpoint_id))?;
            match checkpoint.tree_state() {
                shardtree::TreeState::Empty => w.write_u8(0),
                shardtree::TreeState::AtPosition(pos) => {
                    w.write_u8(1)?;
                    w.write_u64::<LittleEndian>(<u64 as From<Position>>::from(pos))
                }
            }?;
            Vector::write(
                &mut w,
                &checkpoint.marks_removed().iter().collect::<Vec<_>>(),
                |w, mark| w.write_u64::<LittleEndian>(<u64 as From<Position>>::from(**mark)),
            )
        },
    )?;
    Ok(())
}
/// Write memory-backed shardstore, represented tree.
fn write_shardtree<H: Hashable + Clone + Eq + HashSer, C: Ord + std::fmt::Debug + Copy, W: Write>(
    tree: &mut shardtree::ShardTree<
        MemoryShardStore<H, C>,
        COMMITMENT_TREE_LEVELS,
        MAX_SHARD_LEVEL,
    >,
    mut writer: W,
) -> io::Result<()>
where
    u32: From<C>,
{
    // Replace original tree with empty tree, and mutate new version into store.
    let mut store = std::mem::replace(
        tree,
        shardtree::ShardTree::new(MemoryShardStore::empty(), 0),
    )
    .into_store();
    macro_rules! write_with_error_handling {
        ($writer: ident, $from: ident) => {
            if let Err(e) = $writer(&mut writer, &$from) {
                *tree = shardtree::ShardTree::new(store, MAX_REORG);
                return Err(e);
            }
        };
    }
    // Write located prunable trees
    write_with_error_handling!(write_shards, store);
    let mut checkpoints = Vec::new();
    store
        .with_checkpoints(MAX_REORG, |checkpoint_id, checkpoint| {
            checkpoints.push((*checkpoint_id, checkpoint.clone()));
            Ok(())
        })
        .expect("Infallible");
    // Write checkpoints
    write_with_error_handling!(write_checkpoints, checkpoints);
    let cap = store.get_cap().expect("Infallible");
    // Write cap
    write_with_error_handling!(write_shard, cap);
    *tree = shardtree::ShardTree::new(store, MAX_REORG);
    Ok(())
}

impl Default for WitnessTrees {
    fn default() -> WitnessTrees {
        Self {
            witness_tree_sapling: shardtree::ShardTree::new(MemoryShardStore::empty(), MAX_REORG),
            witness_tree_orchard: shardtree::ShardTree::new(MemoryShardStore::empty(), MAX_REORG),
        }
    }
}

impl WitnessTrees {
    pub(crate) fn add_checkpoint(&mut self, height: BlockHeight) {
        self.witness_tree_sapling.checkpoint(height).unwrap();
        self.witness_tree_orchard.checkpoint(height).unwrap();
    }
    const VERSION: u8 = 0;
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let _serialized_version = reader.read_u8()?;
        let witness_tree_sapling = read_shardtree(&mut reader)?;
        let witness_tree_orchard = read_shardtree(reader)?;
        Ok(Self {
            witness_tree_sapling,
            witness_tree_orchard,
        })
    }

    pub fn write<W: Write>(&mut self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        write_shardtree(&mut self.witness_tree_sapling, &mut writer)?;
        write_shardtree(&mut self.witness_tree_orchard, &mut writer)
    }
    pub(crate) fn insert_all_frontier_nodes(
        &mut self,
        non_empty_sapling_frontier: Option<NonEmptyFrontier<sapling::Node>>,
        non_empty_orchard_frontier: Option<NonEmptyFrontier<MerkleHashOrchard>>,
    ) {
        use incrementalmerkletree::Retention;
        if let Some(front) = non_empty_sapling_frontier {
            self.witness_tree_sapling
                .insert_frontier_nodes(front, Retention::Ephemeral)
                .expect("to insert non-empty sapling frontier")
        }
        if let Some(front) = non_empty_orchard_frontier {
            self.witness_tree_orchard
                .insert_frontier_nodes(front, Retention::Ephemeral)
                .expect("to insert non-empty orchard frontier")
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
        Ok(LocatedPrunableTree::from_parts(root_addr, shard))
    })?;
    let mut store = MemoryShardStore::empty();
    for shard in shards {
        store.put_shard(shard).expect("Infallible");
    }
    let checkpoints = Vector::read(&mut reader, |r| {
        let checkpoint_id = C::from(r.read_u32::<LittleEndian>()?);
        let tree_state = match r.read_u8()? {
            0 => shardtree::TreeState::Empty,
            1 => shardtree::TreeState::AtPosition(Position::from(r.read_u64::<LittleEndian>()?)),
            otherwise => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("error reading TreeState: expected boolean value, found {otherwise}"),
                ))
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

impl std::hash::Hash for PoolNullifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PoolNullifier::Sapling(n) => {
                state.write_u8(0);
                n.0.hash(state);
            }
            PoolNullifier::Orchard(n) => {
                state.write_u8(1);
                n.to_bytes().hash(state);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockData {
    pub(crate) ecb: Vec<u8>,
    pub height: u64,
}

impl BlockData {
    pub fn serialized_version() -> u64 {
        20
    }

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

    pub(crate) fn new(mut cb: CompactBlock) -> Self {
        for compact_transaction in &mut cb.vtx {
            for co in &mut compact_transaction.outputs {
                co.ciphertext.clear();
                co.epk.clear();
            }
        }

        cb.header.clear();
        let height = cb.height;

        let mut ecb = vec![];
        cb.encode(&mut ecb).unwrap();

        Self { ecb, height }
    }

    pub(crate) fn cb(&self) -> CompactBlock {
        let b = self.ecb.clone();
        CompactBlock::decode(&b[..]).unwrap()
    }

    pub(crate) fn hash(&self) -> String {
        self.cb().hash().to_string()
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let height = reader.read_i32::<LittleEndian>()? as u64;

        let mut hash_bytes = [0; 32];
        reader.read_exact(&mut hash_bytes)?;
        hash_bytes.reverse();

        // We don't need this, but because of a quirk, the version is stored later, so we can't actually
        // detect the version here. So we write an empty tree and read it back here
        let tree: zcash_primitives::sapling::CommitmentTree = read_commitment_tree(&mut reader)?;
        let _tree = if tree.size() == 0 { None } else { Some(tree) };

        let version = reader.read_u64::<LittleEndian>()?;

        let ecb = if version <= 11 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| r.read_u8())?
        };

        if ecb.is_empty() {
            Ok(BlockData::new_with(height, &hash_bytes))
        } else {
            Ok(BlockData { ecb, height })
        }
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_i32::<LittleEndian>(self.height as i32)?;

        let hash_bytes: Vec<_> = hex::decode(self.hash())
            .unwrap()
            .into_iter()
            .rev()
            .collect();
        writer.write_all(&hash_bytes[..])?;

        write_commitment_tree(
            &CommitmentTree::<zcash_primitives::sapling::Node, 32>::empty(),
            &mut writer,
        )?;
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // Write the ecb as well
        Vector::write(&mut writer, &self.ecb, |w, b| w.write_u8(*b))?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct WitnessCache<Node: Hashable> {
    pub(crate) witnesses: Vec<IncrementalWitness<Node, 32>>,
    pub top_height: u64,
}

impl<Node: Hashable> std::fmt::Debug for WitnessCache<Node> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WitnessCache")
            .field("witnesses", &self.witnesses.len())
            .field("top_height", &self.top_height)
            .finish_non_exhaustive()
    }
}

impl<Node: Hashable> WitnessCache<Node> {
    pub fn new(witnesses: Vec<IncrementalWitness<Node, 32>>, top_height: u64) -> Self {
        Self {
            witnesses,
            top_height,
        }
    }

    pub fn empty() -> Self {
        Self {
            witnesses: vec![],
            top_height: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.witnesses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.witnesses.is_empty()
    }

    pub fn clear(&mut self) {
        self.witnesses.clear();
    }

    pub fn get(&self, i: usize) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.get(i)
    }

    #[cfg(test)]
    pub fn get_from_last(&self, i: usize) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.get(self.len() - i - 1)
    }

    pub fn last(&self) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.last()
    }

    pub fn pop(&mut self, at_height: u64) {
        while !self.witnesses.is_empty() && self.top_height >= at_height {
            self.witnesses.pop();
            self.top_height -= 1;
        }
    }

    // pub fn get_as_string(&self, i: usize) -> String {
    //     if i >= self.witnesses.len() {
    //         return "".to_string();
    //     }

    //     let mut buf = vec![];
    //     self.get(i).unwrap().write(&mut buf).unwrap();
    //     return hex::encode(buf);
    // }
}
pub struct ReceivedSaplingNoteAndMetadata {
    pub diversifier: zcash_primitives::sapling::Diversifier,
    pub note: zcash_primitives::sapling::Note,

    // The postion of this note's commitment
    pub(crate) witnessed_position: Position,

    // The note's index in its containing transaction
    pub(crate) output_index: usize,

    pub(super) nullifier: zcash_primitives::sapling::Nullifier,
    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

#[derive(Debug)]
pub struct ReceivedOrchardNoteAndMetadata {
    pub diversifier: orchard::keys::Diversifier,
    pub note: orchard::note::Note,

    // The postion of this note's commitment
    pub witnessed_position: Position,

    // The note's index in its containing transaction
    pub(crate) output_index: usize,

    pub(super) nullifier: orchard::note::Nullifier,
    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl std::fmt::Debug for ReceivedSaplingNoteAndMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaplingNoteData")
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("is_change", &self.is_change)
            .finish_non_exhaustive()
    }
}

// Reading a note also needs the corresponding address to read from.
pub(crate) fn read_sapling_rseed<R: Read>(
    mut reader: R,
) -> io::Result<zcash_primitives::sapling::Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    let r = match note_type {
        1 => zcash_primitives::sapling::Rseed::BeforeZip212(
            jubjub::Fr::from_bytes(&r_bytes).unwrap(),
        ),
        2 => zcash_primitives::sapling::Rseed::AfterZip212(r_bytes),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Bad note type")),
    };

    Ok(r)
}

pub(crate) fn write_sapling_rseed<W: Write>(
    mut writer: W,
    rseed: &zcash_primitives::sapling::Rseed,
) -> io::Result<()> {
    let note_type = match rseed {
        zcash_primitives::sapling::Rseed::BeforeZip212(_) => 1,
        zcash_primitives::sapling::Rseed::AfterZip212(_) => 2,
    };
    writer.write_u8(note_type)?;

    match rseed {
        zcash_primitives::sapling::Rseed::BeforeZip212(fr) => writer.write_all(&fr.to_bytes()),
        zcash_primitives::sapling::Rseed::AfterZip212(b) => writer.write_all(b),
    }
}

#[derive(Clone, Debug)]
pub struct ReceivedTransparentOutput {
    pub address: String,
    pub txid: TxId,
    pub output_index: u64,
    pub script: Vec<u8>,
    pub value: u64,
    pub height: i32,

    pub spent_at_height: Option<i32>,
    pub spent: Option<TxId>, // If this utxo was confirmed spent

    // If this utxo was spent in a send, but has not yet been confirmed.
    // Contains the txid and height at which the Tx was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
}

impl ReceivedTransparentOutput {
    pub fn serialized_version() -> u64 {
        3
    }

    pub fn to_outpoint(&self) -> OutPoint {
        OutPoint::new(*self.txid.as_ref(), self.output_index as u32)
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;

        let address_len = reader.read_i32::<LittleEndian>()?;
        let mut address_bytes = vec![0; address_len as usize];
        reader.read_exact(&mut address_bytes)?;
        let address = String::from_utf8(address_bytes).unwrap();
        assert_eq!(address.chars().take(1).collect::<Vec<char>>()[0], 't');

        let mut transaction_id_bytes = [0; 32];
        reader.read_exact(&mut transaction_id_bytes)?;
        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let output_index = reader.read_u64::<LittleEndian>()?;
        let value = reader.read_u64::<LittleEndian>()?;
        let height = reader.read_i32::<LittleEndian>()?;

        let script = Vector::read(&mut reader, |r| {
            let mut byte = [0; 1];
            r.read_exact(&mut byte)?;
            Ok(byte[0])
        })?;

        let spent = Optional::read(&mut reader, |r| {
            let mut transaction_bytes = [0u8; 32];
            r.read_exact(&mut transaction_bytes)?;
            Ok(TxId::from_bytes(transaction_bytes))
        })?;

        let spent_at_height = if version <= 1 {
            None
        } else {
            Optional::read(&mut reader, |r| r.read_i32::<LittleEndian>())?
        };

        let unconfirmed_spent = if version <= 2 {
            None
        } else {
            Optional::read(&mut reader, |r| {
                let mut transaction_bytes = [0u8; 32];
                r.read_exact(&mut transaction_bytes)?;

                let height = r.read_u32::<LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_bytes), height))
            })?
        };

        Ok(ReceivedTransparentOutput {
            address,
            txid: transaction_id,
            output_index,
            script,
            value,
            height,
            spent_at_height,
            spent,
            unconfirmed_spent,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        writer.write_u32::<LittleEndian>(self.address.as_bytes().len() as u32)?;
        writer.write_all(self.address.as_bytes())?;

        writer.write_all(self.txid.as_ref())?;

        writer.write_u64::<LittleEndian>(self.output_index)?;
        writer.write_u64::<LittleEndian>(self.value)?;
        writer.write_i32::<LittleEndian>(self.height)?;

        Vector::write(&mut writer, &self.script, |w, b| w.write_all(&[*b]))?;

        Optional::write(&mut writer, self.spent, |w, transaction_id| {
            w.write_all(transaction_id.as_ref())
        })?;

        Optional::write(&mut writer, self.spent_at_height, |w, s| {
            w.write_i32::<LittleEndian>(s)
        })?;

        Optional::write(
            &mut writer,
            self.unconfirmed_spent,
            |w, (transaction_id, height)| {
                w.write_all(transaction_id.as_ref())?;
                w.write_u32::<LittleEndian>(height)
            },
        )?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct OutgoingTxData {
    pub to_address: String,
    pub value: u64,
    pub memo: Memo,
    pub recipient_ua: Option<String>,
}

impl PartialEq for OutgoingTxData {
    fn eq(&self, other: &Self) -> bool {
        self.to_address == other.to_address && self.value == other.value && self.memo == other.memo
    }
}

impl OutgoingTxData {
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let address_len = reader.read_u64::<LittleEndian>()?;
        let mut address_bytes = vec![0; address_len as usize];
        reader.read_exact(&mut address_bytes)?;
        let address = String::from_utf8(address_bytes).unwrap();

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
                format!("Couldn't create memo: {}", e),
            )),
        }?;

        Ok(OutgoingTxData {
            to_address: address,
            value,
            memo,
            recipient_ua: None,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Strings are written as len + utf8
        match &self.recipient_ua {
            None => {
                writer.write_u64::<LittleEndian>(self.to_address.as_bytes().len() as u64)?;
                writer.write_all(self.to_address.as_bytes())?;
            }
            Some(ua) => {
                writer.write_u64::<LittleEndian>(ua.as_bytes().len() as u64)?;
                writer.write_all(ua.as_bytes())?;
            }
        }
        writer.write_u64::<LittleEndian>(self.value)?;
        writer.write_all(self.memo.encode().as_array())
    }
}

pub mod finsight {
    pub struct ValuesSentToAddress(pub std::collections::HashMap<String, Vec<u64>>);
    pub struct TotalValueToAddress(pub std::collections::HashMap<String, u64>);
    pub struct TotalSendsToAddress(pub std::collections::HashMap<String, u64>);
    #[derive(Debug)]
    pub struct TotalMemoBytesToAddress(pub std::collections::HashMap<String, usize>);
    impl From<TotalMemoBytesToAddress> for json::JsonValue {
        fn from(value: TotalMemoBytesToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }
    impl From<TotalValueToAddress> for json::JsonValue {
        fn from(value: TotalValueToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }
    impl From<TotalSendsToAddress> for json::JsonValue {
        fn from(value: TotalSendsToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }
}
pub mod summaries {
    use std::collections::HashMap;

    use json::{object, JsonValue};
    use zcash_primitives::transaction::TxId;

    use crate::wallet::Pool;

    /// The MobileTx is the zingolib representation of
    /// transactions in the format most useful for
    /// consumption in mobile and mobile-like UI
    pub struct ValueTransfer {
        pub block_height: zcash_primitives::consensus::BlockHeight,
        pub datetime: u64,
        pub kind: ValueTransferKind,
        pub memos: Vec<zcash_primitives::memo::TextMemo>,
        pub price: Option<f64>,
        pub txid: TxId,
    }
    impl ValueTransfer {
        pub fn balance_delta(&self) -> i64 {
            use ValueTransferKind::*;
            match self.kind {
                Sent { amount, .. } => -(amount as i64),
                Fee { amount, .. } => -(amount as i64),
                Received { amount, .. } => amount as i64,
                SendToSelf => 0,
            }
        }
    }
    #[derive(Clone)]
    pub enum ValueTransferKind {
        Sent {
            amount: u64,
            to_address: zcash_address::ZcashAddress,
        },
        Received {
            pool: Pool,
            amount: u64,
        },
        SendToSelf,
        Fee {
            amount: u64,
        },
    }
    impl From<&ValueTransferKind> for JsonValue {
        fn from(value: &ValueTransferKind) -> Self {
            match value {
                ValueTransferKind::Sent { .. } => JsonValue::String(String::from("Sent")),
                ValueTransferKind::Received { .. } => JsonValue::String(String::from("Received")),
                ValueTransferKind::SendToSelf => JsonValue::String(String::from("SendToSelf")),
                ValueTransferKind::Fee { .. } => JsonValue::String(String::from("Fee")),
            }
        }
    }
    impl From<ValueTransfer> for JsonValue {
        fn from(value: ValueTransfer) -> Self {
            let mut temp_object = object! {
                    "amount": "",
                    "block_height": u32::from(value.block_height),
                    "datetime": value.datetime,
                    "kind": "",
                    "memos": value.memos.iter().cloned().map(String::from).collect::<Vec<String>>(),
                    "pool": "",
                    "price": value.price,
                    "txid": value.txid.to_string(),
            };
            match value.kind {
                ValueTransferKind::Sent {
                    ref to_address,
                    amount,
                } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["to_address"] = JsonValue::from(to_address.encode());
                    temp_object
                }
                ValueTransferKind::Fee { amount } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object
                }
                ValueTransferKind::Received { pool, amount } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["pool"] = JsonValue::from(pool);
                    temp_object
                }
                ValueTransferKind::SendToSelf => {
                    temp_object["amount"] = JsonValue::from(0);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["pool"] = JsonValue::from("None".to_string());
                    temp_object["price"] = JsonValue::from("None".to_string());
                    temp_object["to_address"] = JsonValue::from("None".to_string());
                    temp_object
                }
            }
        }
    }

    pub struct TransactionIndex(HashMap<zcash_primitives::transaction::TxId, ValueTransfer>);
}
///  Everything (SOMETHING) about a transaction
#[derive(Debug)]
pub struct TransactionMetadata {
    // Block in which this tx was included
    pub block_height: BlockHeight,

    // Is this Tx unconfirmed (i.e., not yet mined)
    pub unconfirmed: bool,

    // Timestamp of Tx. Added in v4
    pub datetime: u64,

    // Txid of this transaction. It's duplicated here (It is also the Key in the HashMap that points to this
    // WalletTx in LightWallet::txs)
    pub txid: TxId,

    // List of all nullifiers spent in this Tx. These nullifiers belong to the wallet.
    pub spent_sapling_nullifiers: Vec<zcash_primitives::sapling::Nullifier>,

    // List of all nullifiers spent in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<orchard::note::Nullifier>,

    // List of all sapling notes received in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<ReceivedSaplingNoteAndMetadata>,

    // List of all sapling notes received in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<ReceivedOrchardNoteAndMetadata>,

    // List of all Utxos received in this Tx. Some of these might be change notes
    pub received_utxos: Vec<ReceivedTransparentOutput>,

    // Total value of all the sapling nullifiers that were spent in this Tx
    pub total_sapling_value_spent: u64,

    // Total value of all the orchard nullifiers that were spent in this Tx
    pub total_orchard_value_spent: u64,

    // Total amount of transparent funds that belong to us that were spent in this Tx.
    pub total_transparent_value_spent: u64,

    // All outgoing sends
    pub outgoing_tx_data: Vec<OutgoingTxData>,

    // Whether this TxID was downloaded from the server and scanned for Memos
    pub full_tx_scanned: bool,

    // Price of Zec when this Tx was created
    pub price: Option<f64>,
}

impl TransactionMetadata {
    pub(super) fn add_spent_nullifier(&mut self, nullifier: PoolNullifier, value: u64) {
        match nullifier {
            PoolNullifier::Sapling(nf) => {
                self.spent_sapling_nullifiers.push(nf);
                self.total_sapling_value_spent += value;
            }
            PoolNullifier::Orchard(nf) => {
                self.spent_orchard_nullifiers.push(nf);
                self.total_orchard_value_spent += value;
            }
        }
    }

    pub fn get_price(datetime: u64, price: &WalletZecPriceInfo) -> Option<f64> {
        match price.zec_price {
            None => None,
            Some((t, p)) => {
                // If the price was fetched within 24 hours of this Tx, we use the "current" price
                // else, we mark it as None, for the historical price fetcher to get
                // TODO:  Investigate the state of "the historical price fetcher".
                if (t as i64 - datetime as i64).abs() < 24 * 60 * 60 {
                    Some(p)
                } else {
                    None
                }
            }
        }
    }

    pub fn get_transaction_fee(&self) -> u64 {
        self.total_value_spent() - (self.value_outgoing() + self.total_change_returned())
    }

    // TODO: This is incorrect in the edge case where where we have a send-to-self with
    // no text memo and 0-value fee
    pub fn is_outgoing_transaction(&self) -> bool {
        (!self.outgoing_tx_data.is_empty()) || self.total_value_spent() != 0
    }
    pub fn is_incoming_transaction(&self) -> bool {
        self.sapling_notes.iter().any(|note| !note.is_change())
            || self.orchard_notes.iter().any(|note| !note.is_change())
            || !self.received_utxos.is_empty()
    }
    pub fn net_spent(&self) -> u64 {
        assert!(self.is_outgoing_transaction());
        self.total_value_spent() - self.total_change_returned()
    }
    pub fn new(
        height: BlockHeight,
        datetime: u64,
        transaction_id: &TxId,
        unconfirmed: bool,
    ) -> Self {
        TransactionMetadata {
            block_height: height,
            unconfirmed,
            datetime,
            txid: *transaction_id,
            spent_sapling_nullifiers: vec![],
            spent_orchard_nullifiers: vec![],
            sapling_notes: vec![],
            orchard_notes: vec![],
            received_utxos: vec![],
            total_transparent_value_spent: 0,
            total_sapling_value_spent: 0,
            total_orchard_value_spent: 0,
            outgoing_tx_data: vec![],
            full_tx_scanned: false,
            price: None,
        }
    }
    pub fn new_txid(txid: &[u8]) -> TxId {
        let mut txid_bytes = [0u8; 32];
        txid_bytes.copy_from_slice(txid);
        TxId::from_bytes(txid_bytes)
    }
    fn pool_change_returned<D: DomainWalletExt>(&self) -> u64
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        D::sum_pool_change(self)
    }

    pub fn pool_value_received<D: DomainWalletExt>(&self) -> u64
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        D::to_notes_vec(self)
            .iter()
            .map(|note_and_metadata| note_and_metadata.value())
            .sum()
    }

    #[allow(clippy::type_complexity)]
    pub fn read<R: Read>(
        mut reader: R,
        (wallet_capability, mut trees): (
            &WalletCapability,
            Option<&mut (
                Vec<(
                    IncrementalWitness<sapling::Node, COMMITMENT_TREE_LEVELS>,
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

        let unconfirmed = if version <= 20 {
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

        tracing::info!("About to attempt to read a note and metadata");
        let sapling_notes = Vector::read_collected_mut(&mut reader, |r| {
            ReceivedSaplingNoteAndMetadata::read(
                r,
                (wallet_capability, trees.as_mut().map(|t| &mut t.0)),
            )
        })?;
        let orchard_notes = if version > 22 {
            Vector::read_collected_mut(&mut reader, |r| {
                ReceivedOrchardNoteAndMetadata::read(
                    r,
                    (wallet_capability, trees.as_mut().map(|t| &mut t.1)),
                )
            })?
        } else {
            vec![]
        };
        let utxos = Vector::read(&mut reader, |r| ReceivedTransparentOutput::read(r))?;

        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        // Outgoing metadata was only added in version 2
        let outgoing_metadata = Vector::read(&mut reader, |r| OutgoingTxData::read(r))?;

        let full_tx_scanned = reader.read_u8()? > 0;

        let zec_price = if version <= 4 {
            None
        } else {
            Optional::read(&mut reader, |r| r.read_f64::<LittleEndian>())?
        };

        let spent_sapling_nullifiers = if version <= 5 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(zcash_primitives::sapling::Nullifier(n))
            })?
        };

        let spent_orchard_nullifiers = if version <= 21 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(orchard::note::Nullifier::from_bytes(&n).unwrap())
            })?
        };

        Ok(Self {
            block_height: block,
            unconfirmed,
            datetime,
            txid: transaction_id,
            sapling_notes,
            orchard_notes,
            received_utxos: utxos,
            spent_sapling_nullifiers,
            spent_orchard_nullifiers,
            total_sapling_value_spent,
            total_transparent_value_spent,
            total_orchard_value_spent,
            outgoing_tx_data: outgoing_metadata,
            full_tx_scanned,
            price: zec_price,
        })
    }

    pub fn serialized_version() -> u64 {
        23
    }

    pub fn total_change_returned(&self) -> u64 {
        self.pool_change_returned::<SaplingDomain<ChainType>>()
            + self.pool_change_returned::<OrchardDomain>()
    }
    pub fn total_value_received(&self) -> u64 {
        self.pool_value_received::<OrchardDomain>()
            + self.pool_value_received::<SaplingDomain<ChainType>>()
            + self
                .received_utxos
                .iter()
                .map(|utxo| utxo.value)
                .sum::<u64>()
    }
    pub fn total_value_spent(&self) -> u64 {
        self.value_spent_by_pool().iter().sum()
    }

    pub fn value_outgoing(&self) -> u64 {
        self.outgoing_tx_data
            .iter()
            .fold(0, |running_total, tx_data| tx_data.value + running_total)
    }

    pub fn value_spent_by_pool(&self) -> [u64; 3] {
        [
            self.total_transparent_value_spent,
            self.total_sapling_value_spent,
            self.total_orchard_value_spent,
        ]
    }
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        let block: u32 = self.block_height.into();
        writer.write_i32::<LittleEndian>(block as i32)?;

        writer.write_u8(if self.unconfirmed { 1 } else { 0 })?;

        writer.write_u64::<LittleEndian>(self.datetime)?;

        writer.write_all(self.txid.as_ref())?;

        Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.received_utxos, |w, u| u.write(w))?;

        for pool in self.value_spent_by_pool() {
            writer.write_u64::<LittleEndian>(pool)?;
        }

        // Write the outgoing metadata
        Vector::write(&mut writer, &self.outgoing_tx_data, |w, om| om.write(w))?;

        writer.write_u8(if self.full_tx_scanned { 1 } else { 0 })?;

        Optional::write(&mut writer, self.price, |w, p| {
            w.write_f64::<LittleEndian>(p)
        })?;

        Vector::write(&mut writer, &self.spent_sapling_nullifiers, |w, n| {
            w.write_all(&n.0)
        })?;
        Vector::write(&mut writer, &self.spent_orchard_nullifiers, |w, n| {
            w.write_all(&n.to_bytes())
        })?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct SpendableSaplingNote {
    pub transaction_id: TxId,
    pub nullifier: zcash_primitives::sapling::Nullifier,
    pub diversifier: zcash_primitives::sapling::Diversifier,
    pub note: zcash_primitives::sapling::Note,
    pub witnessed_position: Position,
    pub extsk: Option<zcash_primitives::zip32::sapling::ExtendedSpendingKey>,
}

#[derive(Debug)]
pub struct SpendableOrchardNote {
    pub transaction_id: TxId,
    pub nullifier: orchard::note::Nullifier,
    pub diversifier: orchard::keys::Diversifier,
    pub note: orchard::note::Note,
    pub witnessed_position: Position,
    pub spend_key: Option<orchard::keys::SpendingKey>,
}

// Struct that tracks the latest and historical price of ZEC in the wallet
#[derive(Clone, Debug)]
pub struct WalletZecPriceInfo {
    // Latest price of ZEC and when it was fetched
    pub zec_price: Option<(u64, f64)>,

    // Wallet's currency. All the prices are in this currency
    pub currency: String,

    // When the last time historical prices were fetched
    pub last_historical_prices_fetched_at: Option<u64>,

    // Historical prices retry count
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
    pub fn serialized_version() -> u64 {
        20
    }

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

        let last_historical_prices_fetched_at =
            Optional::read(&mut reader, |r| r.read_u64::<LittleEndian>())?;
        let historical_prices_retry_count = reader.read_u64::<LittleEndian>()?;

        Ok(Self {
            zec_price,
            currency,
            last_historical_prices_fetched_at,
            historical_prices_retry_count,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // We don't write the currency zec price or the currency yet.
        Optional::write(
            &mut writer,
            self.last_historical_prices_fetched_at,
            |w, t| w.write_u64::<LittleEndian>(t),
        )?;
        writer.write_u64::<LittleEndian>(self.historical_prices_retry_count)?;

        Ok(())
    }
}

#[test]
fn read_write_empty_sapling_tree() {
    let mut buffer = Vec::new();

    write_commitment_tree(
        &CommitmentTree::<zcash_primitives::sapling::Node, 32>::empty(),
        &mut buffer,
    )
    .unwrap();
    assert_eq!(
        CommitmentTree::<zcash_primitives::sapling::Node, 32>::empty(),
        read_commitment_tree(&mut buffer.as_slice()).unwrap()
    )
}
