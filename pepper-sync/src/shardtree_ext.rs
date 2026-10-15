//! Domain-typed outcomes for shardtree's boolean operations.
//!
//! shardtree reports several operations as `Ok(bool)`, where `false`
//! conflates distinct situations that demand different responses. That
//! conflation caused a real defect: a reorg truncation read the `false`
//! from [`ShardTree::truncate_to_checkpoint`] on an empty migrated
//! ironwood tree as corruption and wiped the wallet. This module is the
//! crate's single boundary with those boolean APIs: each wrapper names
//! every outcome in a closed enum, so callers must match on meanings
//! rather than interpret a bit.
//!
//! Raw calls to the wrapped operations are confined to this module. The
//! `raw_boolean_calls_are_confined_to_this_module` test enforces the
//! boundary.

use shardtree::ShardTree;
use shardtree::error::ShardTreeError;
use shardtree::store::memory::MemoryShardStore;
use std::convert::Infallible;

use incrementalmerkletree::Hashable;
use zcash_protocol::consensus::BlockHeight;

/// Outcome of appending a checkpoint that records the tree's current
/// frontier position at a new height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointAppendOutcome {
    /// The association between the id and the current frontier position
    /// was recorded.
    Appended,
    /// The id is not above the store's newest checkpoint. The tree is
    /// unchanged. Appending stamps the *current* frontier, so recording
    /// it at or below an already-stamped height would duplicate or
    /// falsify the height-to-frontier record. Checkpoints for past
    /// heights travel with their own tree states through the
    /// order-agnostic channels instead ([`ShardTree::insert_tree`]'s
    /// per-subtree checkpoint maps, and the store-level
    /// `ShardStore::add_checkpoint`).
    NotAboveNewest,
}

/// Outcome of rolling a tree back to a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackOutcome {
    /// The tree was truncated to the checkpoint's state.
    RolledBack,
    /// No checkpoint exists with the given id. The tree is unchanged.
    NoSuchCheckpoint,
}

/// The boundary wrapper over shardtree's boolean operations, exported so
/// downstream crates route through it too (zingolib's legacy wallet
/// import does. Its own source-walk test enforces its confinement).
pub trait ShardTreeExt {
    /// As [`ShardTree::checkpoint`], with the boolean classified: append
    /// a checkpoint recording the tree's current frontier at the id.
    fn append_checkpoint(
        &mut self,
        checkpoint_id: BlockHeight,
    ) -> Result<CheckpointAppendOutcome, ShardTreeError<Infallible>>;

    /// As [`ShardTree::truncate_to_checkpoint`], with the boolean
    /// classified.
    fn rollback_to_checkpoint(
        &mut self,
        checkpoint_id: BlockHeight,
    ) -> Result<RollbackOutcome, ShardTreeError<Infallible>>;
}

impl<H, const DEPTH: u8, const SHARD_HEIGHT: u8> ShardTreeExt
    for ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>
where
    H: Hashable + Clone + PartialEq,
{
    fn append_checkpoint(
        &mut self,
        checkpoint_id: BlockHeight,
    ) -> Result<CheckpointAppendOutcome, ShardTreeError<Infallible>> {
        Ok(if self.checkpoint(checkpoint_id)? {
            CheckpointAppendOutcome::Appended
        } else {
            CheckpointAppendOutcome::NotAboveNewest
        })
    }

    fn rollback_to_checkpoint(
        &mut self,
        checkpoint_id: BlockHeight,
    ) -> Result<RollbackOutcome, ShardTreeError<Infallible>> {
        Ok(if self.truncate_to_checkpoint(&checkpoint_id)? {
            RollbackOutcome::RolledBack
        } else {
            RollbackOutcome::NoSuchCheckpoint
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The boundary rule: every raw call to a wrapped boolean operation
    /// lives in this module. A raw call elsewhere reintroduces the
    /// boolean-conflation class this module retires, so the tree walk
    /// fails the build the moment one appears.
    #[test]
    fn raw_boolean_calls_are_confined_to_this_module() {
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut rust_sources = Vec::new();
        collect_rust_sources(&src_root, &mut rust_sources);
        assert!(!rust_sources.is_empty());

        let raw_patterns = [
            ".checkpoint(",
            ".truncate_to_checkpoint(",
            ".truncate_to_checkpoint_depth(",
        ];
        for path in rust_sources {
            if path.ends_with("shardtree_ext.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for pattern in raw_patterns {
                assert!(
                    !source.contains(pattern),
                    "{} calls a raw shardtree boolean operation ({pattern}); \
                     route it through crate::shardtree_ext instead",
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

    /// The wrappers report the store's actual behavior: a fresh tree
    /// accepts an initial checkpoint, refuses a non-advancing one, rolls
    /// back to an existing checkpoint, and reports a missing one.
    #[test]
    fn outcomes_match_store_behavior() {
        let mut tree: ShardTree<
            MemoryShardStore<orchard::tree::MerkleHashOrchard, BlockHeight>,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
            { crate::witness::SHARD_HEIGHT },
        > = ShardTree::new(MemoryShardStore::empty(), 10);

        let five = BlockHeight::from_u32(5);
        assert_eq!(
            tree.append_checkpoint(five).unwrap(),
            CheckpointAppendOutcome::Appended
        );
        assert_eq!(
            tree.append_checkpoint(five).unwrap(),
            CheckpointAppendOutcome::NotAboveNewest
        );
        assert_eq!(
            tree.rollback_to_checkpoint(five).unwrap(),
            RollbackOutcome::RolledBack
        );
        assert_eq!(
            tree.rollback_to_checkpoint(BlockHeight::from_u32(4))
                .unwrap(),
            RollbackOutcome::NoSuchCheckpoint
        );
    }
}
