//! The truncation contract: two pure decision rules, applied where the
//! data lives.
//!
//! [`plan_truncation`] routes the wallet-level decision, from the
//! wallet state ([`WalletTruncationState`]: birthday and highest
//! scanned height) and the truncation target, it returns whether the
//! truncation is a no-op, a full clear, or a rollback to the target.
//! `plan_pool_truncation` is the per-tree rule: from one tree's
//! checkpoint facts ([`TreeTruncationFacts`]) it returns that tree's
//! outcome, and the shard-tree applier derives each tree's outcome
//! through it at the point of application. Both rules are pure. All
//! mutation stays with the appliers.
//!
//! Every per-pool outcome carries the evidence that justifies it: a
//! rollback carries the checkpoint it found, and a rescan sentence
//! carries the newest checkpoint that condemned the tree. The applier
//! derives its actions and its error messages from that evidence
//! instead of re-deriving or asserting it.
//!
//! The contract the plan guarantees, per store:
//!
//! - Wallet blocks, transactions, nullifiers, and outpoints retain
//!   exactly the data at or below the plan's height.
//! - A shard tree holding a checkpoint at the height rolls back to it.
//! - A shard tree recording no chain state above the height is left
//!   untouched: it has nothing to remove, so it cannot be "broken" with
//!   respect to this truncation. This is the state of the empty ironwood
//!   tree a pre-ironwood (v0) wallet blob migrates to, whose only
//!   checkpoint is the height-zero initialization checkpoint.
//! - A shard tree holding state above the height with no checkpoint at
//!   it cannot roll back. Only the clear-and-rescan recovery restores
//!   integrity, and the plan says so explicitly.
//!
//! Decisions live here and nowhere else: the applying code performs no
//! judgement of its own beyond reporting a planned rollback that the
//! tree store unexpectedly refuses.

use incrementalmerkletree::Hashable;
use shardtree::ShardTree;
use shardtree::store::ShardStore;
use shardtree::store::memory::MemoryShardStore;
use zcash_protocol::consensus::{self, BlockHeight};

/// The wallet state a truncation decision reads: where scanning began
/// and how far it has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletTruncationState {
    /// The wallet birthday. A target below it means no retained state
    /// can anchor the wallet, so every store resets.
    pub birthday: BlockHeight,
    /// The highest block height that has been scanned. A target above
    /// it leaves nothing to remove.
    pub highest_scanned_height: BlockHeight,
}

/// What the planner reads of one shard tree, gathered at the truncation
/// target.
///
/// A checkpoint records every *scanned* chain state the tree has
/// absorbed (frontier insertion and scanned-subtree insertion attach
/// checkpoints), so the newest checkpoint bounds the scanned state.
/// Server-fetched subtree roots are the exception: they enter the store
/// with no checkpoint, so this planner cannot see them. They are kept
/// safe by a different mechanism: each sync session refetches the
/// newest still-bare root and replaces it if the chain moved (see
/// `crate::witness::subtree_fetch_start_index`), so a stale bare root
/// surviving an `Untouched` verdict is healed at the next session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTruncationFacts {
    /// The checkpoint the tree's store holds exactly at the truncation
    /// target, if any.
    pub checkpoint_at_target: Option<BlockHeight>,
    /// The newest checkpoint id the store holds, if any.
    pub newest_checkpoint: Option<BlockHeight>,
}

/// Reads one tree's truncation facts from its store.
pub(crate) fn tree_facts<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
    truncate_height: BlockHeight,
) -> TreeTruncationFacts
where
    H: Hashable + Clone + PartialEq,
{
    TreeTruncationFacts {
        checkpoint_at_target: tree
            .store()
            .get_checkpoint(&truncate_height)
            .expect("infallible")
            .map(|_| truncate_height),
        newest_checkpoint: tree.store().max_checkpoint_id().expect("infallible"),
    }
}

/// The correct truncation outcome for one shard tree, carrying the
/// evidence that justifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolTruncation {
    /// The tree holds this checkpoint at the target height: roll back
    /// to it.
    ToCheckpoint {
        /// The checkpoint found at the target height.
        checkpoint: BlockHeight,
    },
    /// The tree records no chain state above the target height (its
    /// newest checkpoint, if any, is at or below it): leave it untouched.
    Untouched,
    /// The tree holds state above the target height but no checkpoint at
    /// it: rollback is impossible, and only a full clear and rescan
    /// restores integrity.
    RequiresRescan {
        /// The newest checkpoint above the target that condemned
        /// the tree.
        newest_checkpoint: BlockHeight,
    },
}

/// The correct post-truncation state of every store, decided purely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationPlan {
    /// The target lies above the highest scanned block: every store is
    /// already correct and nothing is removed.
    NoOp,
    /// The target lies below the wallet birthday (or is height zero):
    /// no retained state can anchor the wallet, so every store (blocks,
    /// transactions, nullifiers, outpoints, and all three shard
    /// trees) resets to empty.
    ClearAll,
    /// Roll the wallet back: blocks, transactions, nullifiers, and
    /// outpoints retain exactly the data at or below `height`, and each
    /// shard tree applies the per-pool outcome `plan_pool_truncation`
    /// derives at the point of application.
    Truncate {
        /// The height retained. Everything strictly above it goes.
        height: BlockHeight,
    },
}

/// Decides the correct truncation of the wallet, purely.
///
/// See the module documentation for the contract the returned plan
/// guarantees per store.
#[must_use]
pub fn plan_truncation(
    wallet_state: WalletTruncationState,
    truncate_height: BlockHeight,
) -> TruncationPlan {
    if truncate_height == consensus::H0 || truncate_height < wallet_state.birthday {
        return TruncationPlan::ClearAll;
    }
    if truncate_height > wallet_state.highest_scanned_height {
        return TruncationPlan::NoOp;
    }
    TruncationPlan::Truncate {
        height: truncate_height,
    }
}

/// Decides the correct truncation of one shard tree, purely, from the
/// evidence in its facts.
pub(crate) fn plan_pool_truncation(
    facts: TreeTruncationFacts,
    truncate_height: BlockHeight,
) -> PoolTruncation {
    if let Some(checkpoint) = facts.checkpoint_at_target {
        return PoolTruncation::ToCheckpoint { checkpoint };
    }
    match facts.newest_checkpoint {
        Some(newest_checkpoint) if newest_checkpoint > truncate_height => {
            PoolTruncation::RequiresRescan { newest_checkpoint }
        }
        _ => PoolTruncation::Untouched,
    }
}

/// Decides whether one shard tree rolls back to the wallet's highest
/// scanned height at the start of a sync session, purely, from the
/// evidence in its facts gathered at that height.
///
/// A newest checkpoint above the highest scanned height is the evidence
/// of state from blocks the wallet has yet to record as scanned, which
/// a sync session leaves behind when it ends partway through a wallet
/// update. Such a tree rolls back to its checkpoint at the highest
/// scanned height, and the checkpoint is returned.
///
/// Every other tree is left as it is. A tree whose newest checkpoint is
/// at or below the highest scanned height has nothing to remove. A tree
/// that holds a newer checkpoint and none at the highest scanned height
/// cannot roll back, and its state above that height only conflicts
/// with scanning when a re-org replaces the blocks it came from. A pool
/// rescan leaves the trees of the earlier pools in this state.
pub(crate) fn plan_unscanned_state_rollback(
    facts: TreeTruncationFacts,
    highest_scanned_height: BlockHeight,
) -> Option<BlockHeight> {
    facts.checkpoint_at_target.filter(|_| {
        facts
            .newest_checkpoint
            .is_some_and(|newest_checkpoint| newest_checkpoint > highest_scanned_height)
    })
}

#[cfg(test)]
mod test;

/// Property tests over generated checkpoint sets, complementing the
/// exhaustive facts-level sweep in `test`: the facts here are derived
/// from a whole generated set, as `gather` would derive them from a
/// store, so the properties quantify over tree histories rather than
/// pre-digested facts.
#[cfg(test)]
mod properties;
