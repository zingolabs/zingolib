use super::traits::{self, DomainWalletExt, ToBytes};
use crate::error::{ZingoLibError, ZingoLibResult};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::frontier::{CommitmentTree, NonEmptyFrontier};
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::{Address, Hashable, Level, Position};
use orchard::note_encryption::OrchardDomain;
use orchard::tree::MerkleHashOrchard;
use prost::Message;
use sapling_crypto::note_encryption::SaplingDomain;
use sapling_crypto::Node;
use shardtree::store::memory::MemoryShardStore;
use shardtree::store::{Checkpoint, ShardStore};
use shardtree::LocatedPrunableTree;
use shardtree::ShardTree;
use std::convert::TryFrom;
use std::io::{self, Read, Write};
use std::usize;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_backend::serialization::shardtree::{read_shard, write_shard};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::merkle_tree::{read_commitment_tree, write_commitment_tree, HashSer};
use zcash_primitives::{memo::Memo, transaction::TxId};
use zingoconfig::MAX_REORG;

pub const COMMITMENT_TREE_LEVELS: u8 = 32;
pub const MAX_SHARD_LEVEL: u8 = 16;

/// This type is motivated by the IPC architecture where (currently) channels traffic in
/// `(TxId, WalletNullifier, BlockHeight, Option<u32>)`.  This enum permits a single channel
/// type to handle nullifiers from different domains.
/// <https://github.com/zingolabs/zingolib/issues/64>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolNullifier {
    Sapling(sapling_crypto::Nullifier),
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
                shardtree::store::TreeState::Empty => w.write_u8(0),
                shardtree::store::TreeState::AtPosition(pos) => {
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
        non_empty_sapling_frontier: Option<NonEmptyFrontier<Node>>,
        non_empty_orchard_frontier: Option<NonEmptyFrontier<MerkleHashOrchard>>,
    ) {
        self.insert_domain_frontier_nodes::<SaplingDomain>(non_empty_sapling_frontier);
        self.insert_domain_frontier_nodes::<OrchardDomain>(non_empty_orchard_frontier);
    }
    fn insert_domain_frontier_nodes<D: DomainWalletExt>(
        &mut self,
        non_empty_frontier: Option<
            NonEmptyFrontier<<D::WalletNote as super::notes::ShieldedNoteInterface>::Node>,
        >,
    ) where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        use incrementalmerkletree::Retention;
        if let Some(front) = non_empty_frontier {
            D::get_shardtree_mut(self)
                .insert_frontier_nodes(front, Retention::Ephemeral)
                .unwrap_or_else(|e| {
                    let _: ZingoLibResult<()> = ZingoLibError::Error(format!(
                        "failed to insert non-empty {} frontier: {e}",
                        D::NAME
                    ))
                    .handle();
                })
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default()
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
            0 => shardtree::store::TreeState::Empty,
            1 => shardtree::store::TreeState::AtPosition(Position::from(
                r.read_u64::<LittleEndian>()?,
            )),
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
                co.ephemeral_key.clear();
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
        let tree: sapling_crypto::CommitmentTree = read_commitment_tree(&mut reader)?;
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
            &CommitmentTree::<sapling_crypto::Node, 32>::empty(),
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

// Reading a note also needs the corresponding address to read from.
pub(crate) fn read_sapling_rseed<R: Read>(mut reader: R) -> io::Result<sapling_crypto::Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    let r = match note_type {
        1 => sapling_crypto::Rseed::BeforeZip212(jubjub::Fr::from_bytes(&r_bytes).unwrap()),
        2 => sapling_crypto::Rseed::AfterZip212(r_bytes),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Bad note type")),
    };

    Ok(r)
}

pub(crate) fn write_sapling_rseed<W: Write>(
    mut writer: W,
    rseed: &sapling_crypto::Rseed,
) -> io::Result<()> {
    let note_type = match rseed {
        sapling_crypto::Rseed::BeforeZip212(_) => 1,
        sapling_crypto::Rseed::AfterZip212(_) => 2,
    };
    writer.write_u8(note_type)?;

    match rseed {
        sapling_crypto::Rseed::BeforeZip212(fr) => writer.write_all(&fr.to_bytes()),
        sapling_crypto::Rseed::AfterZip212(b) => writer.write_all(b),
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
        (self.to_address == other.to_address || self.recipient_ua == other.recipient_ua)
            && self.value == other.value
            && self.memo == other.memo
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
    #[derive(PartialEq)]
    pub struct ValueTransfer {
        pub block_height: zcash_primitives::consensus::BlockHeight,
        pub datetime: u64,
        pub kind: ValueTransferKind,
        pub memos: Vec<zcash_primitives::memo::TextMemo>,
        pub price: Option<f64>,
        pub txid: TxId,
        pub unconfirmed: bool,
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

    impl std::fmt::Debug for ValueTransfer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            use core::ops::Deref as _;
            f.debug_struct("ValueTransfer")
                .field("block_height", &self.block_height)
                .field("datetime", &self.datetime)
                .field("kind", &self.kind)
                .field(
                    "memos",
                    &self
                        .memos
                        .iter()
                        .map(zcash_primitives::memo::TextMemo::deref)
                        .collect::<Vec<_>>(),
                )
                .field("price", &self.price)
                .field("txid", &self.txid)
                .field("unconfirmed", &self.unconfirmed)
                .finish()
        }
    }

    #[derive(Clone, PartialEq, Eq, Debug)]
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
                    "unconfirmed": value.unconfirmed,
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
}

pub use crate::wallet::transaction_record::TransactionRecord;

#[test]
#[cfg(feature = "test-features")]
fn single_transparent_note_makes_is_incoming_true() {
    // A single transparent note makes is_incoming_trsaction true.
    let txid = TxId::from_bytes([0u8; 32]);
    let transparent_note = crate::test_framework::TransparentNoteBuilder::new()
        .address("t".to_string())
        .spent(Some((txid, 3)))
        .build();
    let mut transaction_record = TransactionRecord::new(
        zingo_status::confirmation_status::ConfirmationStatus::Confirmed(BlockHeight::from_u32(5)),
        1705077003,
        &txid,
    );
    transaction_record.transparent_notes.push(transparent_note);
    assert!(transaction_record.is_incoming_transaction());
}
#[derive(Debug)]
pub struct SpendableSaplingNote {
    pub transaction_id: TxId,
    pub nullifier: sapling_crypto::Nullifier,
    pub diversifier: sapling_crypto::Diversifier,
    pub note: sapling_crypto::Note,
    pub witnessed_position: Position,
    pub extsk: Option<sapling_crypto::zip32::ExtendedSpendingKey>,
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
        &CommitmentTree::<sapling_crypto::Node, 32>::empty(),
        &mut buffer,
    )
    .unwrap();
    assert_eq!(
        CommitmentTree::<sapling_crypto::Node, 32>::empty(),
        read_commitment_tree(&mut buffer.as_slice()).unwrap()
    )
}
