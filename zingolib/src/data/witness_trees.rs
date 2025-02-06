//! TODO: Add Mod Description Here!

use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::frontier::NonEmptyFrontier;
use incrementalmerkletree::{Address, Hashable, Level, Position};
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use sapling_crypto::{note_encryption::SaplingDomain, Node};
use shardtree::{
    store::{memory::MemoryShardStore, Checkpoint, ShardStore},
    LocatedPrunableTree, ShardTree,
};

use zcash_client_backend::proto::service::TreeState;
use zcash_client_backend::serialization::shardtree::{read_shard, write_shard};
use zcash_encoding::Vector;
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::merkle_tree::HashSer;

use crate::config::MAX_REORG;

/// TODO: Add Doc Comment Here!
pub const COMMITMENT_TREE_LEVELS: u8 = 32;
/// TODO: Add Doc Comment Here!
pub const MAX_SHARD_LEVEL: u8 = 16;

use crate::error::{ZingoLibError, ZingoLibResult};
use crate::wallet::notes::ShieldedNoteInterface;
use crate::wallet::{
    notes,
    traits::{self, DomainWalletExt},
};

pub(crate) type SapStore = MemoryShardStore<Node, BlockHeight>;
pub(crate) type OrchStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub struct WitnessTrees {
    /// TODO: Add Doc Comment Here!
    pub witness_tree_sapling: ShardTree<SapStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
    /// TODO: Add Doc Comment Here!
    pub witness_tree_orchard: ShardTree<OrchStore, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>,
}

pub mod trait_walletcommitmenttrees;

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
    /// helper truncates shard trees for both shielded pools
    pub(crate) fn truncate_to_checkpoint(&mut self, checkpoint_height: BlockHeight) {
        self.witness_tree_sapling
            .truncate_to_checkpoint(&checkpoint_height)
            .expect("Infallible");
        self.witness_tree_orchard
            .truncate_to_checkpoint(&checkpoint_height)
            .expect("Infallible");
    }

    const VERSION: u8 = 0;

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

    /// TODO: Add Doc Comment Here!
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
            NonEmptyFrontier<<D::WalletNote as ShieldedNoteInterface>::Node>,
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

    /// TODO: Add Doc Comment Here!
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

/// TODO: Add Doc Comment Here!
pub fn get_legacy_frontiers(
    trees: TreeState,
) -> (
    Option<incrementalmerkletree::frontier::NonEmptyFrontier<sapling_crypto::Node>>,
    Option<incrementalmerkletree::frontier::NonEmptyFrontier<MerkleHashOrchard>>,
) {
    (
        get_legacy_frontier::<SaplingDomain>(&trees),
        get_legacy_frontier::<OrchardDomain>(&trees),
    )
}

fn get_legacy_frontier<D: DomainWalletExt>(
    trees: &TreeState,
) -> Option<
    incrementalmerkletree::frontier::NonEmptyFrontier<
        <D::WalletNote as notes::ShieldedNoteInterface>::Node,
    >,
>
where
    <D as Domain>::Note: PartialEq + Clone,
    <D as Domain>::Recipient: traits::Recipient,
{
    zcash_primitives::merkle_tree::read_commitment_tree::<
        <D::WalletNote as notes::ShieldedNoteInterface>::Node,
        &[u8],
        COMMITMENT_TREE_LEVELS,
    >(&hex::decode(D::get_tree(trees)).unwrap()[..])
    .ok()
    .and_then(|tree| tree.to_frontier().take())
}
