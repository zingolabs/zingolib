use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::local_consensus::LocalNetwork;

/// A local network with every upgrade active from `height`.
const fn activated_at(height: u32) -> LocalNetwork {
    let activation = Some(BlockHeight::from_u32(height));
    LocalNetwork {
        overwinter: activation,
        sapling: activation,
        blossom: activation,
        heartwood: activation,
        canopy: activation,
        nu5: activation,
        nu6: activation,
        nu6_1: activation,
        nu6_2: activation,
        nu6_3: activation,
        nu7: activation,
    }
}

/// A local network with every upgrade active from block 1.
const NETWORK: LocalNetwork = activated_at(1);

/// The completion contract of [`crate::sync::SyncStatus::is_complete`]:
/// completion is the sync task's own terminal condition (sync has
/// started and every scan range is `Scanned`), independent of the
/// output ratio, so an output-free birthday-to-chain-height range can
/// complete and a stale `total_outputs` cannot fake completion.
mod sync_status_completion {
    use zcash_protocol::consensus::BlockHeight;

    use crate::sync::{ScanPriority, ScanRange, SyncStatus};

    /// Builds a status with the given start height and scan-range
    /// priorities. Every counter stays zero: completion must be
    /// decided by the scan ranges alone, never by the output ratio.
    fn status(sync_start_height: u32, priorities: &[ScanPriority]) -> SyncStatus {
        let scan_ranges = priorities
            .iter()
            .enumerate()
            .map(|(index, priority)| {
                let start = 1_000 + 10 * index as u32;
                ScanRange::from_parts(
                    BlockHeight::from(start)..BlockHeight::from(start + 10),
                    *priority,
                )
            })
            .collect();
        SyncStatus {
            scan_ranges,
            sync_start_height: sync_start_height.into(),
            session_blocks_scanned: 0,
            total_blocks_scanned: 0,
            total_blocks: 0,
            session_sapling_outputs_scanned: 0,
            total_sapling_outputs_scanned: 0,
            session_orchard_outputs_scanned: 0,
            total_orchard_outputs_scanned: 0,
            session_ironwood_outputs_scanned: 0,
            total_ironwood_outputs_scanned: 0,
            total_outputs_scanned: 0,
            total_outputs: 0,
        }
    }

    #[test]
    fn never_started_is_not_complete() {
        assert!(!status(0, &[]).is_complete());
    }

    /// A started sync with no scan ranges left in the state is
    /// vacuously complete: nothing remains tracked, so nothing
    /// awaits scanning or nullifier work.
    #[test]
    fn started_with_no_scan_ranges_is_complete() {
        assert!(status(1_000, &[]).is_complete());
    }

    #[test]
    fn all_ranges_scanned_is_complete() {
        assert!(status(1_000, &[ScanPriority::Scanned, ScanPriority::Scanned]).is_complete());
    }

    /// The empty-range edge: zero shielded outputs from birthday to
    /// chain height must not read as incomplete once the sync task
    /// has run to its terminal state.
    #[test]
    fn output_free_range_is_complete() {
        let status = status(1_000, &[ScanPriority::Scanned]);
        assert_eq!(status.total_outputs, 0);
        assert!(status.is_complete());
    }

    #[test]
    fn nullifier_retrieval_pending_is_not_complete() {
        for pending in [
            ScanPriority::ScannedWithoutMapping,
            ScanPriority::RefetchingNullifiers,
        ] {
            assert!(!status(1_000, &[ScanPriority::Scanned, pending]).is_complete());
        }
    }

    /// The stale-denominator edge: an unscanned range keeps the
    /// status incomplete even if the output counters claim the
    /// initially-computed target was reached.
    #[test]
    fn unscanned_range_is_not_complete() {
        let mut status = status(1_000, &[ScanPriority::Scanned, ScanPriority::Historic]);
        status.total_outputs_scanned = 10_000;
        status.total_outputs = 10_000;
        assert!(!status.is_complete());
    }
}

/// The truncation contract of [`crate::sync::truncate_wallet_data`]:
/// a reorg truncation rolls every store back to the truncate height,
/// and a shard tree that records nothing above that height (such as
/// the empty ironwood tree a pre-ironwood (v0) wallet blob migrates
/// to) is untouched, never classified broken. Only a tree that
/// holds state above the height and cannot roll back to it forces
/// the clear-and-rescan path.
mod truncation {
    use crate::engine::{ScanTargetOps, SyncStateOps, WalletBlockOps};
    use std::collections::BTreeMap;

    use zcash_primitives::{block::BlockHash, transaction::TxId};
    use zcash_protocol::consensus::BlockHeight;

    use crate::mocks::MockWalletBuilder;
    use crate::shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt};
    use crate::sync::{ScanPriority, ScanRange, truncate_wallet_data};
    use crate::wallet::{
        ScanTarget, ShardTrees, SyncState, TreeBounds, WalletBlock,
        traits::{SyncBlocks, SyncWallet},
    };

    /// A wallet block carrying only what truncation reads: its height.
    fn block(height: u32) -> WalletBlock {
        WalletBlock::from_parts(
            BlockHeight::from_u32(height),
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
        )
    }

    /// A wallet synced through height 10 with a birthday of 6: one
    /// fully scanned range, one wallet block per scanned height, and
    /// the given shard trees.
    fn synced_wallet(shard_trees: ShardTrees) -> crate::mocks::MockWallet {
        let wallet_blocks: BTreeMap<_, _> = (6..=10u32)
            .map(|height| (BlockHeight::from_u32(height), block(height)))
            .collect();
        let sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(6)..BlockHeight::from_u32(11),
            ScanPriority::Scanned,
        )]);
        MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(6))
            .sync_state(sync_state)
            .wallet_blocks(wallet_blocks)
            .shard_trees(shard_trees)
            .create_mock_wallet()
    }

    /// A pre-ironwood (v0) wallet blob deserializes to an ironwood
    /// tree holding only the initialization checkpoint at height
    /// zero (`ShardTrees::read`), while sapling and orchard carry
    /// the checkpoints of past scanning. The first routine reorg
    /// truncation after the upgrade must roll sapling and orchard
    /// back and leave the empty ironwood tree untouched, not
    /// classify it broken and wipe the wallet.
    #[test]
    fn migrated_wallet_survives_reorg_truncation() {
        // Sapling and orchard hold checkpoints for the scanned
        // heights; the ironwood tree stays exactly as
        // `ShardTrees::new` built it (its height-zero initialization
        // checkpoint and nothing else), which is also the state
        // `ShardTrees::read` produces for a pre-ironwood blob.
        let mut shard_trees = ShardTrees::new();
        for height in 6..=10u32 {
            assert_eq!(
                shard_trees
                    .sapling
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
            assert_eq!(
                shard_trees
                    .orchard
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
        }
        let mut wallet = synced_wallet(shard_trees);
        let kept_target = ScanTarget::internal(BlockHeight::from_u32(7), TxId::from_bytes([7; 32]));
        let evicted_target =
            ScanTarget::internal(BlockHeight::from_u32(9), TxId::from_bytes([9; 32]));
        for scan_target in [kept_target, evicted_target] {
            wallet
                .get_sync_state_mut()
                .unwrap()
                .insert_scan_target(scan_target);
        }

        // A routine two-block reorg rolls the wallet back to height 8.
        let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

        assert!(
            result.is_ok(),
            "reorg truncation wiped a healthy migrated wallet: {result:?}"
        );
        // The blocks at and below the truncate height survive; the
        // clear-and-rescan path leaves none.
        assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_ok());
        // A target above the truncation names a transaction in an evicted block and goes with it.
        assert_eq!(
            wallet
                .get_sync_state()
                .unwrap()
                .scan_targets()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [kept_target]
        );
        assert!(wallet.get_wallet_block(BlockHeight::from_u32(8)).is_ok());
        assert!(wallet.get_wallet_block(BlockHeight::from_u32(9)).is_err());
    }

    /// A tree that records state above the truncate height but holds
    /// no checkpoint at it cannot roll back. The established recovery
    /// (clear all wallet data and rescan) is preserved for it.
    #[test]
    fn unrecoverable_tree_still_clears_wallet_data() {
        // Orchard scanned past the target but its checkpoint at the
        // target height is gone (e.g. pruned): checkpoints exist only
        // above it.
        let mut shard_trees = ShardTrees::new();
        for height in 6..=10u32 {
            assert_eq!(
                shard_trees
                    .sapling
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
        }
        for height in 9..=10u32 {
            assert_eq!(
                shard_trees
                    .orchard
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
        }
        let mut wallet = synced_wallet(shard_trees);

        let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

        assert!(matches!(
            result,
            Err(crate::error::SyncError::TruncationError(_, _))
        ));
        // The wallet was cleared for rescan.
        assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_err());
    }

    /// The balance path reads the newest sapling checkpoint as the anchor.
    fn assert_every_tree_holds_a_checkpoint(wallet: &mut crate::mocks::MockWallet) {
        use crate::wallet::traits::SyncShardTrees;
        use shardtree::store::ShardStore as _;

        let shard_trees = wallet.get_shard_trees_mut().unwrap();
        let sapling = shard_trees.sapling.store().max_checkpoint_id().unwrap();
        let orchard = shard_trees.orchard.store().max_checkpoint_id().unwrap();
        let ironwood = shard_trees.ironwood.store().max_checkpoint_id().unwrap();
        assert!(
            sapling.is_some() && orchard.is_some() && ironwood.is_some(),
            "a shard tree lost its last checkpoint: sapling {sapling:?}, orchard {orchard:?}, ironwood {ironwood:?}"
        );
    }

    /// A target below the birthday clears every store but keeps a checkpoint.
    #[test]
    fn clear_all_truncation_leaves_a_checkpoint_in_every_tree() {
        let mut shard_trees = ShardTrees::new();
        for height in 6..=10u32 {
            shard_trees
                .sapling
                .append_checkpoint(BlockHeight::from_u32(height))
                .unwrap();
        }
        let mut wallet = synced_wallet(shard_trees);

        truncate_wallet_data(&mut wallet, BlockHeight::from_u32(3)).unwrap();

        assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_err());
        assert_every_tree_holds_a_checkpoint(&mut wallet);
    }

    /// The rescan recovery must also leave a checkpoint in every tree.
    #[test]
    fn rescan_recovery_leaves_a_checkpoint_in_every_tree() {
        let mut shard_trees = ShardTrees::new();
        for height in 9..=10u32 {
            shard_trees
                .orchard
                .append_checkpoint(BlockHeight::from_u32(height))
                .unwrap();
        }
        let mut wallet = synced_wallet(shard_trees);

        let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

        assert!(matches!(
            result,
            Err(crate::error::SyncError::TruncationError(_, _))
        ));
        assert_every_tree_holds_a_checkpoint(&mut wallet);
    }
}

/// The repair [`crate::sync::rollback_unscanned_tree_states`] applies at the start of a sync session: a shard
/// tree holding state from blocks above the highest scanned height rolls back to its checkpoint at this height,
/// and every other tree is left as it is.
mod unscanned_shard_tree_state {
    use std::convert::Infallible;

    use incrementalmerkletree::{Marking, Position, Retention};
    use orchard::tree::MerkleHashOrchard;
    use shardtree::error::ShardTreeError;
    use shardtree::store::{Checkpoint, ShardStore as _};
    use zcash_protocol::consensus::BlockHeight;
    use zl_netutils::lightwallet_protocol::SubtreeRoot;

    use crate::mocks::{MockWallet, MockWalletBuilder};
    use crate::sync::{ScanPriority, ScanRange, rollback_unscanned_tree_states};
    use crate::wallet::{ShardTrees, SyncState, traits::SyncShardTrees};
    use crate::witness;

    const BIRTHDAY: u32 = 6;
    const HIGHEST_SCANNED: u32 = 10;
    const NEXT_BLOCK: u32 = HIGHEST_SCANNED + 1;
    /// Each block holds one orchard note commitment, so the block above the highest scanned height inserts here.
    const NEXT_BLOCK_POSITION: u64 = (NEXT_BLOCK - BIRTHDAY) as u64;

    const SCANNED_CHAIN: u8 = 1;
    const STALE_BLOCK: u8 = 2;
    const REPLACING_BLOCK: u8 = 3;

    /// A note commitment that differs for each `chain_tag`.
    fn note_commitment(chain_tag: u8) -> MerkleHashOrchard {
        let mut bytes = [0; 32];
        bytes[0] = chain_tag;
        MerkleHashOrchard::from_bytes(&bytes).expect("a small value is a valid note commitment")
    }

    /// The note commitment of the block at `height` with the checkpoint scanning gives the last note commitment
    /// of a block.
    fn block_leaf(chain_tag: u8, height: u32) -> (MerkleHashOrchard, Retention<BlockHeight>) {
        (
            note_commitment(chain_tag),
            Retention::Checkpoint {
                id: BlockHeight::from_u32(height),
                marking: Marking::None,
            },
        )
    }

    /// Shard trees with the orchard note commitments of every block from the birthday to the highest scanned
    /// height.
    fn scanned_shard_trees() -> ShardTrees {
        let mut shard_trees = ShardTrees::new();
        for height in BIRTHDAY..=HIGHEST_SCANNED {
            let (leaf, retention) = block_leaf(SCANNED_CHAIN, height);
            shard_trees.orchard.append(leaf, retention).unwrap();
        }

        shard_trees
    }

    /// Inserts the orchard note commitment of the block above the highest scanned height as scan results do.
    fn insert_next_block(
        shard_trees: &mut ShardTrees,
        chain_tag: u8,
    ) -> Result<(), ShardTreeError<Infallible>> {
        const LOCATED_TREE_SIZE: usize = 1;
        for located_tree in witness::build_located_trees(
            Position::from(NEXT_BLOCK_POSITION),
            vec![block_leaf(chain_tag, NEXT_BLOCK)],
            LOCATED_TREE_SIZE,
        ) {
            shard_trees
                .orchard
                .insert_tree(located_tree.subtree, located_tree.checkpoints)?;
        }

        Ok(())
    }

    /// A wallet scanned from the birthday to the highest scanned height with the block above still to be
    /// scanned.
    fn wallet(shard_trees: ShardTrees) -> MockWallet {
        let sync_state = SyncState::new_for_test(vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(NEXT_BLOCK),
                ScanPriority::Scanned,
            ),
            ScanRange::from_parts(
                BlockHeight::from_u32(NEXT_BLOCK)..BlockHeight::from_u32(NEXT_BLOCK + 1),
                ScanPriority::Verify,
            ),
        ]);
        MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(BIRTHDAY))
            .sync_state(sync_state)
            .shard_trees(shard_trees)
            .create_mock_wallet()
    }

    /// A sync session ended after it inserted the note commitments of the block above the highest scanned
    /// height and before it recorded the block as scanned. A re-org then replaced the block, so scanning it
    /// again conflicts with the stale note commitment. The rollback removes the stale note commitment and its
    /// checkpoint, and the replacing block inserts.
    #[test]
    fn stale_block_is_rolled_back_and_its_replacement_inserts() {
        let mut shard_trees = scanned_shard_trees();
        insert_next_block(&mut shard_trees, STALE_BLOCK).unwrap();
        assert!(
            insert_next_block(&mut shard_trees, REPLACING_BLOCK).is_err(),
            "the replacing block must conflict with the stale block for this test to hold"
        );
        let mut wallet = wallet(shard_trees);

        rollback_unscanned_tree_states(&mut wallet).unwrap();

        let shard_trees = wallet.get_shard_trees_mut().unwrap();
        assert_eq!(
            shard_trees.orchard.max_leaf_position(None).unwrap(),
            Some(Position::from(NEXT_BLOCK_POSITION - 1)),
            "only the note commitments of scanned blocks are kept"
        );
        assert_eq!(
            shard_trees.orchard.store().max_checkpoint_id().unwrap(),
            Some(BlockHeight::from_u32(HIGHEST_SCANNED))
        );
        insert_next_block(shard_trees, REPLACING_BLOCK)
            .expect("the replacing block inserts into the rolled back tree");
    }

    /// A wallet whose shard trees hold only scanned blocks keeps the subtree roots above its highest scanned
    /// height, which a wallet that is behind the chain tip holds.
    #[test]
    fn scanned_shard_trees_are_left_as_they_are() {
        const SUBTREE_ABOVE_HIGHEST_SCANNED: usize = 1;
        let mut shard_trees = scanned_shard_trees();
        witness::add_subtree_roots(
            SUBTREE_ABOVE_HIGHEST_SCANNED,
            vec![SubtreeRoot {
                root_hash: note_commitment(SCANNED_CHAIN).to_bytes().to_vec(),
                ..Default::default()
            }],
            &mut shard_trees.orchard,
        )
        .unwrap();
        let mut wallet = wallet(shard_trees);

        rollback_unscanned_tree_states(&mut wallet).unwrap();

        assert_eq!(
            witness::stored_subtree_root_count(&wallet.get_shard_trees_mut().unwrap().orchard),
            SUBTREE_ABOVE_HIGHEST_SCANNED + 1
        );
    }

    /// The orchard pool has no note commitments in the scanned blocks or in the block above them, so the
    /// checkpoints of these blocks point to the last note commitment of a block below the scanned range, which
    /// the tree is yet to hold. shardtree refuses to roll back to such a checkpoint, so the session starts with
    /// the tree as it is.
    #[test]
    fn shard_tree_that_cannot_roll_back_is_left_as_it_is() {
        let mut shard_trees = ShardTrees::new();
        for height in [HIGHEST_SCANNED, NEXT_BLOCK] {
            shard_trees
                .orchard
                .store_mut()
                .add_checkpoint(
                    BlockHeight::from_u32(height),
                    Checkpoint::at_position(Position::from(NEXT_BLOCK_POSITION - 1)),
                )
                .unwrap();
        }
        let mut wallet = wallet(shard_trees);

        rollback_unscanned_tree_states(&mut wallet).unwrap();

        let shard_trees = wallet.get_shard_trees_mut().unwrap();
        assert_eq!(
            shard_trees.orchard.store().max_checkpoint_id().unwrap(),
            Some(BlockHeight::from_u32(NEXT_BLOCK))
        );
    }
}

/// The pool-accounting contract of [`crate::sync::sync_status`]:
/// every output total on the status (the per-pool u32 fields and
/// the u64 exact-ratio pair) describes the same per-pool trio,
/// summed once through `output_pool_total`. Regression for the
/// skew where the u64 fields re-summed only sapling and orchard
/// while the percentages, then derived here, included ironwood.
/// Each pool contributes a distinct count, so dropping any pool
/// from any consumer changes an asserted value.
mod sync_status_pool_accounting {
    use crate::engine::{InitialSyncStateOps, SyncStateOps, WalletBlockOps};
    use std::collections::BTreeMap;

    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;

    use crate::mocks::MockWalletBuilder;
    use crate::sync::{ScanPriority, ScanRange, sync_status};
    use crate::wallet::{InitialSyncState, SyncState, TreeBounds, WalletBlock};

    const NOTHING_PREVIOUSLY_SCANNED: u32 = 0;

    /// A wallet block carrying only what tree-bounds accounting
    /// reads: its height and tree sizes.
    fn block(height: u32, tree_bounds: TreeBounds) -> WalletBlock {
        WalletBlock::from_parts(
            BlockHeight::from_u32(height),
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            tree_bounds,
        )
    }

    /// Tree bounds whose initial and final sizes coincide, for
    /// blocks that only mark a boundary of a scanned range.
    fn flat_bounds(sapling: u32, orchard: u32, ironwood: u32) -> TreeBounds {
        TreeBounds {
            sapling_initial_tree_size: sapling,
            sapling_final_tree_size: sapling,
            orchard_initial_tree_size: orchard,
            orchard_final_tree_size: orchard,
            ironwood_initial_tree_size: ironwood,
            ironwood_final_tree_size: ironwood,
        }
    }

    #[tokio::test]
    async fn exact_ratio_fields_agree_with_per_pool_totals() {
        // One fully scanned range whose boundary blocks yield a
        // distinct scanned count per pool: sapling 3, orchard 5,
        // ironwood 7.
        let mut sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(1_000)..BlockHeight::from_u32(1_010),
            ScanPriority::Scanned,
        )]);
        // Session denominators: 10 sapling + 20 orchard + 40
        // ironwood outputs between the wallet bounds, 70 in all.
        sync_state.set_initial_sync_state(InitialSyncState::from_parts(
            BlockHeight::from_u32(1_000),
            TreeBounds {
                sapling_initial_tree_size: 100,
                sapling_final_tree_size: 110,
                orchard_initial_tree_size: 200,
                orchard_final_tree_size: 220,
                ironwood_initial_tree_size: 300,
                ironwood_final_tree_size: 340,
            },
            NOTHING_PREVIOUSLY_SCANNED,
            1,
            2,
            3,
        ));

        let wallet_blocks = BTreeMap::from([
            (
                BlockHeight::from_u32(1_000),
                block(1_000, flat_bounds(100, 200, 300)),
            ),
            (
                BlockHeight::from_u32(1_009),
                block(1_009, flat_bounds(103, 205, 307)),
            ),
        ]);

        let wallet = MockWalletBuilder::new()
            .sync_state(sync_state)
            .wallet_blocks(wallet_blocks)
            .create_mock_wallet();

        let status = sync_status(&wallet).await.unwrap();

        assert_eq!(status.total_sapling_outputs_scanned, 3);
        assert_eq!(status.total_orchard_outputs_scanned, 5);
        assert_eq!(status.total_ironwood_outputs_scanned, 7);

        // The regression proper: the u64 exact-ratio fields must
        // equal the sum of the per-pool fields reported beside
        // them. Under the skew, total_outputs_scanned was 8 (no
        // ironwood) and total_outputs was 30.
        assert_eq!(
            status.total_outputs_scanned,
            u64::from(status.total_sapling_outputs_scanned)
                + u64::from(status.total_orchard_outputs_scanned)
                + u64::from(status.total_ironwood_outputs_scanned),
        );
        assert_eq!(status.total_outputs_scanned, 15);
        assert_eq!(status.total_outputs, 70);
    }

    /// HYPOTHESIS: `sync_status` reports a hand-built state whose
    /// session baseline exceeds its span, with the ratio unreduced and
    /// the session count saturated, instead of taking its caller down.
    /// Falsified while a session percentage divided by that span.
    #[tokio::test]
    async fn status_survives_a_baseline_above_the_span() {
        let mut sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(1_000)..BlockHeight::from_u32(1_010),
            ScanPriority::Scanned,
        )]);
        // The span: ten orchard outputs between the recorded tree bounds.
        // A baseline above that span, which the engine never records.
        sync_state.set_initial_sync_state(InitialSyncState::from_parts(
            BlockHeight::from_u32(1_000),
            TreeBounds {
                sapling_initial_tree_size: 100,
                sapling_final_tree_size: 100,
                orchard_initial_tree_size: 200,
                orchard_final_tree_size: 210,
                ironwood_initial_tree_size: 300,
                ironwood_final_tree_size: 300,
            },
            NOTHING_PREVIOUSLY_SCANNED,
            NOTHING_PREVIOUSLY_SCANNED,
            70,
            NOTHING_PREVIOUSLY_SCANNED,
        ));

        let wallet_blocks = BTreeMap::from([
            (
                BlockHeight::from_u32(1_000),
                block(1_000, flat_bounds(100, 200, 300)),
            ),
            (
                BlockHeight::from_u32(1_009),
                block(1_009, flat_bounds(100, 275, 300)),
            ),
        ]);
        let wallet = MockWalletBuilder::new()
            .sync_state(sync_state)
            .wallet_blocks(wallet_blocks)
            .create_mock_wallet();

        let status = sync_status(&wallet)
            .await
            .expect("an inconsistent state is a reading, not a failure");

        // The exact ratio is reported unreduced.
        assert_eq!(status.total_outputs_scanned, 75);
        assert_eq!(status.total_outputs, 10);
        // The session count saturates rather than underflowing.
        assert_eq!(status.session_orchard_outputs_scanned, 5);
    }
}

/// The progress contract of [`crate::sync::sync_status`] under a
/// stale initial sync state, whose recorded previously-scanned
/// counts stand at or above the whole span the wallet can scan:
/// the total counts report the scanned span, untouched by the
/// stale session baseline.
mod sync_status_stale_initial_state {
    use crate::engine::{InitialSyncStateOps, SyncStateOps, WalletBlockOps};
    use std::collections::BTreeMap;

    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;

    use crate::mocks::MockWalletBuilder;
    use crate::sync::{ScanPriority, ScanRange, sync_status};
    use crate::wallet::{InitialSyncState, SyncState, TreeBounds, WalletBlock};

    /// The wallet birthday, and so the first height of the span.
    const BIRTHDAY_HEIGHT: u32 = 1_000;
    /// The number of blocks between the birthday and the chain tip.
    const TOTAL_BLOCKS: u32 = 10;
    /// The number of those blocks this wallet has scanned.
    const SCANNED_BLOCKS: u32 = 5;
    /// The height of the chain tip the wallet last knew.
    const CHAIN_TIP_HEIGHT: u32 = BIRTHDAY_HEIGHT + TOTAL_BLOCKS - 1;
    /// The first height beyond the scanned range.
    const SCANNED_RANGE_END: u32 = BIRTHDAY_HEIGHT + SCANNED_BLOCKS;
    /// The factor by which a stale count exceeds the true span.
    const STALENESS_FACTOR: u32 = 500;
    /// A previously-scanned count left over from a wider span.
    const STALE_PREVIOUSLY_SCANNED_BLOCKS: u32 = TOTAL_BLOCKS * STALENESS_FACTOR;

    /// The sapling tree size at the start of the wallet's span.
    const SAPLING_INITIAL_TREE_SIZE: u32 = 100;
    /// The sapling outputs the whole span holds.
    const SAPLING_TOTAL_OUTPUTS: u32 = 10;
    /// The sapling outputs the wallet has scanned.
    const SAPLING_SCANNED_OUTPUTS: u32 = 3;
    /// The orchard tree size at the start of the wallet's span.
    const ORCHARD_INITIAL_TREE_SIZE: u32 = 200;
    /// The orchard outputs the whole span holds.
    const ORCHARD_TOTAL_OUTPUTS: u32 = 20;
    /// The orchard outputs the wallet has scanned.
    const ORCHARD_SCANNED_OUTPUTS: u32 = 5;
    /// The ironwood tree size at the start of the wallet's span.
    const IRONWOOD_INITIAL_TREE_SIZE: u32 = 300;
    /// The ironwood outputs the whole span holds.
    const IRONWOOD_TOTAL_OUTPUTS: u32 = 40;
    /// The ironwood outputs the wallet has scanned.
    const IRONWOOD_SCANNED_OUTPUTS: u32 = 7;
    /// The outputs the whole span holds, across every pool.
    const TOTAL_OUTPUTS: u32 =
        SAPLING_TOTAL_OUTPUTS + ORCHARD_TOTAL_OUTPUTS + IRONWOOD_TOTAL_OUTPUTS;
    /// The outputs the wallet has scanned, across every pool.
    const SCANNED_OUTPUTS: u32 =
        SAPLING_SCANNED_OUTPUTS + ORCHARD_SCANNED_OUTPUTS + IRONWOOD_SCANNED_OUTPUTS;
    /// A previously-scanned output count left over from a wider span.
    const STALE_PREVIOUSLY_SCANNED_OUTPUTS: u32 = TOTAL_OUTPUTS * STALENESS_FACTOR;

    /// A wallet block carrying only what tree-bounds accounting
    /// reads: its height and tree sizes.
    fn block(height: u32, tree_bounds: TreeBounds) -> WalletBlock {
        WalletBlock::from_parts(
            BlockHeight::from_u32(height),
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            tree_bounds,
        )
    }

    /// Tree bounds whose initial and final sizes coincide, for
    /// blocks that only mark a boundary of a scanned range.
    fn flat_bounds(sapling: u32, orchard: u32, ironwood: u32) -> TreeBounds {
        TreeBounds {
            sapling_initial_tree_size: sapling,
            sapling_final_tree_size: sapling,
            orchard_initial_tree_size: orchard,
            orchard_final_tree_size: orchard,
            ironwood_initial_tree_size: ironwood,
            ironwood_final_tree_size: ironwood,
        }
    }

    /// A wallet holding a half-scanned span whose initial sync
    /// state records previously-scanned counts far above that
    /// span, the shape a truncated or rewound wallet leaves
    /// behind.
    fn stale_state_wallet() -> crate::mocks::MockWallet {
        let mut sync_state = SyncState::new_for_test(vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY_HEIGHT)..BlockHeight::from_u32(SCANNED_RANGE_END),
                ScanPriority::Scanned,
            ),
            ScanRange::from_parts(
                BlockHeight::from_u32(SCANNED_RANGE_END)
                    ..BlockHeight::from_u32(CHAIN_TIP_HEIGHT + 1),
                ScanPriority::Historic,
            ),
        ]);
        sync_state.set_initial_sync_state(InitialSyncState::from_parts(
            BlockHeight::from_u32(BIRTHDAY_HEIGHT),
            TreeBounds {
                sapling_initial_tree_size: SAPLING_INITIAL_TREE_SIZE,
                sapling_final_tree_size: SAPLING_INITIAL_TREE_SIZE + SAPLING_TOTAL_OUTPUTS,
                orchard_initial_tree_size: ORCHARD_INITIAL_TREE_SIZE,
                orchard_final_tree_size: ORCHARD_INITIAL_TREE_SIZE + ORCHARD_TOTAL_OUTPUTS,
                ironwood_initial_tree_size: IRONWOOD_INITIAL_TREE_SIZE,
                ironwood_final_tree_size: IRONWOOD_INITIAL_TREE_SIZE + IRONWOOD_TOTAL_OUTPUTS,
            },
            STALE_PREVIOUSLY_SCANNED_BLOCKS,
            STALE_PREVIOUSLY_SCANNED_OUTPUTS,
            STALE_PREVIOUSLY_SCANNED_OUTPUTS,
            STALE_PREVIOUSLY_SCANNED_OUTPUTS,
        ));

        let wallet_blocks = BTreeMap::from([
            (
                BlockHeight::from_u32(BIRTHDAY_HEIGHT),
                block(
                    BIRTHDAY_HEIGHT,
                    flat_bounds(
                        SAPLING_INITIAL_TREE_SIZE,
                        ORCHARD_INITIAL_TREE_SIZE,
                        IRONWOOD_INITIAL_TREE_SIZE,
                    ),
                ),
            ),
            (
                BlockHeight::from_u32(SCANNED_RANGE_END - 1),
                block(
                    SCANNED_RANGE_END - 1,
                    flat_bounds(
                        SAPLING_INITIAL_TREE_SIZE + SAPLING_SCANNED_OUTPUTS,
                        ORCHARD_INITIAL_TREE_SIZE + ORCHARD_SCANNED_OUTPUTS,
                        IRONWOOD_INITIAL_TREE_SIZE + IRONWOOD_SCANNED_OUTPUTS,
                    ),
                ),
            ),
        ]);

        MockWalletBuilder::new()
            .sync_state(sync_state)
            .wallet_blocks(wallet_blocks)
            .create_mock_wallet()
    }

    /// HYPOTHESIS: a stale session baseline leaves the total block
    /// counts at the scanned span, so the falsifier is a status whose
    /// total block counts disagree with it while half the span is
    /// unscanned.
    #[tokio::test]
    async fn stale_block_state_reports_total_progress() {
        let wallet = stale_state_wallet();

        let status = sync_status(&wallet).await.unwrap();

        assert_eq!(status.total_blocks_scanned, SCANNED_BLOCKS);
        assert_eq!(status.total_blocks, TOTAL_BLOCKS);
    }

    /// HYPOTHESIS: the total output counts obey the same rule as the
    /// total block counts, so the falsifier is a status whose total
    /// output counts disagree with the scanned span while most of the
    /// span's outputs are unscanned.
    #[tokio::test]
    async fn stale_output_state_reports_total_progress() {
        let wallet = stale_state_wallet();

        let status = sync_status(&wallet).await.unwrap();

        assert_eq!(status.total_outputs_scanned, u64::from(SCANNED_OUTPUTS));
        assert_eq!(status.total_outputs, u64::from(TOTAL_OUTPUTS));
    }
}

/// The mode a completed scan leaves behind, exercised as a table on the pure step and once through the atomic.
mod on_completion {
    use std::sync::atomic::AtomicU8;

    use crate::wallet::SyncMode;

    #[test]
    fn running_sync_is_shutdown() {
        assert_eq!(SyncMode::Running.on_completion(), SyncMode::Shutdown);
    }

    #[test]
    fn paused_sync_stays_paused() {
        assert_eq!(SyncMode::Paused.on_completion(), SyncMode::Paused);
    }

    #[test]
    fn shutdown_requested_by_the_consumer_is_kept() {
        assert_eq!(SyncMode::Shutdown.on_completion(), SyncMode::Shutdown);
    }

    #[test]
    fn sync_that_is_not_running_is_left_alone() {
        assert_eq!(SyncMode::NotRunning.on_completion(), SyncMode::NotRunning);
    }

    #[test]
    fn apply_stores_the_step_and_returns_the_mode_it_replaced() {
        let sync_mode = AtomicU8::new(SyncMode::Running as u8);

        let before = SyncMode::apply(&sync_mode, SyncMode::on_completion).unwrap();

        assert_eq!(before, SyncMode::Running);
        assert_eq!(
            SyncMode::from_atomic_u8(&sync_mode).unwrap(),
            SyncMode::Shutdown
        );
    }

    #[test]
    fn transition_leaves_another_mode_in_place_and_reports_it() {
        let sync_mode = AtomicU8::new(SyncMode::Paused as u8);

        let before =
            SyncMode::transition(&sync_mode, SyncMode::Running, SyncMode::Shutdown).unwrap();

        assert_eq!(before, SyncMode::Paused);
        assert_eq!(
            SyncMode::from_atomic_u8(&sync_mode).unwrap(),
            SyncMode::Paused
        );
    }
}

/// The drain policy for scanner shutdown, exercised as a table:
/// pure inputs, no runtime, no clocks.
mod drain_verdict {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32},
        },
        time::Duration,
    };

    use zl_netutils::time::MEMPOOL_DRAIN_CEILING;

    use crate::sync::{MempoolDrainVerdict, mempool_drain_verdict};

    /// One row of the drain-policy table:
    /// (workers, unprocessed, connected_for, poll_elapsed, verdict, label).
    type DrainCase = (
        Arc<AtomicBool>,
        Arc<AtomicU32>,
        Duration,
        Option<Duration>,
        MempoolDrainVerdict,
        &'static str,
    );

    #[tokio::test]
    async fn table() {
        let cases: &[DrainCase] = &[
            // The reported bug: first-loop shutdown on a fully
            // synced chain, stream not yet connected. Hold the
            // session open instead of closing it instantly.
            (
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU32::new(0)),
                Duration::from_millis(0),
                None,
                MempoolDrainVerdict::NotShutdown,
                "no stream yet",
            ),
            // Connected but inside the settle window: waits before checking whether there is work to process.
            (
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU32::new(0)),
                Duration::from_millis(100),
                Some(Duration::from_millis(50)),
                MempoolDrainVerdict::ShutdownAndDrainComplete,
                "settling",
            ),
            // The typical session: stream connected long ago,
            // scanner drained. Immediate shutdown, no added cost.
            (
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU32::new(0)),
                Duration::from_millis(0),
                Some(Duration::from_millis(1_400)),
                MempoolDrainVerdict::ShutdownAndDrainComplete,
                "settled and drained",
            ),
            // Unprocessed work always re-enters the processing
            // loop, whatever the stream state. No deadline caps
            // the processing itself.
            (
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU32::new(3)),
                Duration::from_millis(3_000),
                Some(Duration::from_millis(1_400)),
                MempoolDrainVerdict::ShutdownNotDrained,
                "unprocessed work",
            ),
            // Ceiling with a stream that never connected: the
            // pre-c90f8d309 semantics. A dead stream must not
            // hold the session open.
            // Written as the ceiling itself: a literal chosen against
            // one value of it stops testing the ceiling the moment the
            // constant moves.
            (
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU32::new(0)),
                MEMPOOL_DRAIN_CEILING,
                None,
                MempoolDrainVerdict::ShutdownAndDrainComplete,
                "ceiling without stream",
            ),
        ];
        for (shutdown_mempool, unprocessed, connected_ms, elapsed_ms, expected, name) in cases {
            assert_eq!(
                mempool_drain_verdict(
                    shutdown_mempool.clone(),
                    unprocessed.clone(),
                    *connected_ms,
                    *elapsed_ms,
                )
                .await,
                *expected,
                "{name}"
            );

            // TODO: assert shutdown_mempool is true is cases where mempool should be shutdown
        }
    }
}

/// The lifecycle of a note's spend mark across spend detection and `reset_spends`.
///
/// The wallet remembers an on-chain spend observation in exactly one durable place: the
/// note's `spending_transaction` field. The nullifier map is a transient rendezvous
/// buffer: detection only reads it, and its entries are pruned behind the
/// fully-scanned frontier (`remove_irrelevant_data`). These tests pin the
/// consequences for `set_transactions_failed`, whose `reset_spends` call erases that
/// one durable place.
mod spend_reset_lifecycle {
    use crate::engine::ScanTargetOps;
    use std::collections::HashMap;

    use sapling_crypto::value::NoteValue;
    use zcash_keys::keys::UnifiedSpendingKey;
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::{
        consensus::{BlockHeight, MAIN_NETWORK},
        memo::Memo,
    };
    use zl_common::status::ConfirmationStatus;

    use crate::{
        mocks::{MockWallet, MockWalletBuilder},
        sync::{set_transactions_failed, spend},
        wallet::{
            NullifierMap, OutputId, OutputInterface as _, ScanTarget, WalletNote,
            WalletTransaction,
            traits::{SyncNullifiers, SyncTransactions},
        },
    };

    pub(super) const FUNDING_HEIGHT: BlockHeight = BlockHeight::from_u32(10);
    pub(super) const SPEND_HEIGHT: BlockHeight = BlockHeight::from_u32(100);
    pub(super) const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);
    pub(super) const SPENDING_TXID: TxId = TxId::from_bytes([2; 32]);
    pub(super) const NOTE_NULLIFIER: sapling_crypto::Nullifier =
        sapling_crypto::Nullifier([42; 32]);

    /// The spending transaction's wallet record in the given lifecycle state.
    fn spending_record(status: ConfirmationStatus) -> WalletTransaction {
        WalletTransaction::new_for_test(SPENDING_TXID, status)
    }

    /// A confirmed funding transaction holding one sapling note with a derived
    /// nullifier, optionally already marked spent.
    ///
    /// The crypto-note construction duplicates `zingolib::mocks::SaplingCryptoNoteBuilder`,
    /// which cannot be used here (zingolib depends on this crate). Relocating the note
    /// builders down into this crate is a deferred follow-up.
    /// The spending key of the wallet's only account, which received the funding note.
    pub(super) fn spending_key() -> UnifiedSpendingKey {
        UnifiedSpendingKey::from_seed(&MAIN_NETWORK, &[0; 32], zip32::AccountId::ZERO).unwrap()
    }

    pub(super) fn funding_transaction(spending_transaction: Option<TxId>) -> WalletTransaction {
        let (_, recipient) = spending_key().sapling().default_address();
        let crypto_note = sapling_crypto::Note::from_parts(
            recipient,
            NoteValue::from_raw(100_000),
            sapling_crypto::Rseed::AfterZip212([0; 32]),
        );
        let mut note = WalletNote::new_for_test(
            OutputId::new(FUNDING_TXID, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            crypto_note,
            Memo::Empty,
            None,
        )
        .with_nullifier_for_test(NOTE_NULLIFIER);
        note.set_spending_transaction(spending_transaction);

        WalletTransaction::new_for_test(FUNDING_TXID, ConfirmationStatus::Confirmed(FUNDING_HEIGHT))
            .with_sapling_notes_for_test(vec![note])
    }

    fn get_spending_txid(wallet: &MockWallet) -> Option<TxId> {
        wallet
            .get_wallet_transactions()
            .unwrap()
            .get(&FUNDING_TXID)
            .unwrap()
            .sapling_notes()
            .first()
            .unwrap()
            .spending_transaction()
    }

    /// The detection pass `locate_shielded_spends` and `apply_shielded_spends` perform,
    /// minus the network round trips and the scan prioritisation: match derived note
    /// nullifiers against the wallet's nullifier map and mark the matches spent.
    fn run_spend_detection(wallet: &mut MockWallet) {
        let (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers) =
            spend::collect_derived_nullifiers(wallet.get_wallet_transactions().unwrap().values());
        let spend_scan_targets = spend::detect_shielded_spends(
            wallet.get_nullifiers().unwrap(),
            &sapling_nullifiers,
            &orchard_nullifiers,
            &ironwood_nullifiers,
        );
        spend::update_spent_notes(wallet, spend_scan_targets, true).unwrap();
    }

    /// A spend reset *before* the spending transaction's block is scanned heals.
    /// When the scanner later reaches the block, `collect_nullifiers` maps the spend
    /// and detection re-marks the note.
    #[test]
    fn reset_before_scan_heals_on_spend_detection() {
        let mut wallet_transactions = HashMap::new();
        wallet_transactions.insert(FUNDING_TXID, funding_transaction(None));
        wallet_transactions.insert(
            SPENDING_TXID,
            spending_record(ConfirmationStatus::Failed(SPEND_HEIGHT)),
        );
        let mut nullifier_map = NullifierMap::new();
        nullifier_map.sapling.insert(
            NOTE_NULLIFIER,
            ScanTarget::internal(SPEND_HEIGHT, SPENDING_TXID),
        );
        let mut wallet = MockWalletBuilder::new()
            .wallet_transactions(wallet_transactions)
            .nullifier_map(nullifier_map)
            .create_mock_wallet();

        run_spend_detection(&mut wallet);

        assert_eq!(get_spending_txid(&wallet), Some(SPENDING_TXID));
        // Detection only reads the map: the entry stays until cleanup prunes it, so a
        // detection pass that fails part way through is repeated on the next scan.
        assert!(
            wallet
                .get_nullifiers()
                .unwrap()
                .sapling
                .contains_key(&NOTE_NULLIFIER)
        );
    }

    /// Failure-marking a transaction that is `Confirmed` on chain must not
    /// destroy the wallet's knowledge of the spend.
    ///
    /// Once cleanup has pruned the nullifier-map entry behind the fully-scanned
    /// frontier, the note's `spending_transaction` field is the wallet's only durable
    /// record of the on-chain spend. If `set_transactions_failed` erased it, the note
    /// would become a permanent phantom unspent note: every future proposal selects it
    /// and is rejected as a double-spend, and no forward sync can correct it.
    ///
    /// The damage is permanent once the spending block has been scanned and the
    /// fully-scanned frontier has passed it, and the record is `Confirmed` from scan
    /// time on. If the reset lands while the entry is still mapped, the next detection
    /// pass re-observes the nullifier and heals the wallet
    /// (see [`reset_before_scan_heals_on_spend_detection`]). A `Confirmed` record is
    /// therefore the earliest point at which the harm can occur, which is why the
    /// failure path is guarded on `Confirmed` status.
    ///
    /// `set_transactions_failed` therefore refuses to fail a `Confirmed` transaction:
    /// the record keeps its status and the spend mark survives. Only truncation may
    /// invalidate mined transactions (via `set_transactions_failed_unchecked`),
    /// because it simultaneously reopens the affected scan ranges.
    #[test]
    fn failing_a_confirmed_transaction_must_not_destroy_the_spend_observation() {
        let mut wallet_transactions = HashMap::new();
        wallet_transactions.insert(FUNDING_TXID, funding_transaction(Some(SPENDING_TXID)));
        wallet_transactions.insert(
            SPENDING_TXID,
            spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT)),
        );
        // The map entry was pruned by cleanup behind the fully-scanned frontier: empty map.
        let mut wallet = MockWalletBuilder::new()
            .wallet_transactions(wallet_transactions)
            .create_mock_wallet();

        // A late failure marking (e.g. an expiry decision racing the scanner) targets
        // a transaction that is confirmed on chain.
        set_transactions_failed(
            wallet.get_wallet_transactions_mut().unwrap(),
            vec![SPENDING_TXID],
        );

        // A mined transaction cannot fail: the record keeps its `Confirmed` status.
        assert_eq!(
            wallet
                .get_wallet_transactions()
                .unwrap()
                .get(&SPENDING_TXID)
                .unwrap()
                .status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );

        // The spend must remain detectable: after a detection pass the note is still
        // marked spent.
        run_spend_detection(&mut wallet);
        assert_eq!(
            get_spending_txid(&wallet),
            Some(SPENDING_TXID),
            "the on-chain spend observation was destroyed: \
                 the note is now a permanent phantom unspent note"
        );
    }

    /// A scanned transaction record replaces an existing `Failed` record
    /// wholesale, because `extend_wallet_transactions` merges via `HashMap::extend`.
    /// This is why the reset-before-scan sequence heals completely.
    #[test]
    fn scanned_transaction_overwrites_failed_record() {
        let mut wallet_transactions = HashMap::new();
        wallet_transactions.insert(
            SPENDING_TXID,
            spending_record(ConfirmationStatus::Failed(SPEND_HEIGHT)),
        );
        let mut wallet = MockWalletBuilder::new()
            .wallet_transactions(wallet_transactions)
            .create_mock_wallet();

        wallet
            .extend_wallet_transactions(HashMap::from([(
                SPENDING_TXID,
                spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT)),
            )]))
            .unwrap();

        assert_eq!(
            wallet
                .get_wallet_transactions()
                .unwrap()
                .get(&SPENDING_TXID)
                .unwrap()
                .status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );
    }
}

/// Transparent spend detection must leave the spending transaction confirmed in the wallet.
///
/// Compact block scanning maps the transparent inputs of every transaction above the transparent scan floor but
/// only targets a transaction for a full scan when one of its outputs pays the wallet. A transaction that spends
/// a coin in full to an external recipient is therefore known to the wallet only as the pending record created
/// when it was sent.
mod transparent_spend_without_change {
    use crate::engine::{ScanTargetOps, TransparentCoinOps, WalletBlockOps};
    use std::collections::{BTreeMap, HashMap};

    use tokio::sync::mpsc;
    use zcash_primitives::{block::BlockHash, transaction::TxId};
    use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};
    use zcash_transparent::{address::Script, keys::NonHardenedChildIndex};
    use zl_common::status::ConfirmationStatus;
    use zl_netutils::lightwallet_protocol::RawTransaction;

    use crate::{
        client::FetchRequest,
        error::SyncError,
        keys::transparent::{TransparentAddressId, TransparentScope},
        mocks::{MockWallet, MockWalletBuilder, MockWalletError},
        sync::spend,
        wallet::{
            OutputId, OutputInterface as _, ScanTarget, TransparentCoin, TreeBounds, WalletBlock,
            WalletTransaction,
            traits::{SyncOutPoints, SyncTransactions},
        },
    };

    use super::NETWORK;
    const FUNDING_HEIGHT: BlockHeight = BlockHeight::from_u32(10);
    const SPEND_HEIGHT: BlockHeight = BlockHeight::from_u32(100);
    const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);

    /// The spending transaction's wallet record in the given lifecycle state, keyed by the txid the server
    /// returns it under.
    fn spending_record(status: ConfirmationStatus) -> WalletTransaction {
        let txid = WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
            .transaction()
            .txid();
        WalletTransaction::new_for_test(txid, status)
    }

    /// A wallet holding a confirmed coin, the given record of the transaction that spends it, and the spend's
    /// outpoint as mapped from the compact block at `SPEND_HEIGHT`.
    fn wallet_with_mapped_spend(spending_record: WalletTransaction) -> MockWallet {
        let coin_id = OutputId::new(FUNDING_TXID, 0);
        let coin = TransparentCoin::from_parts(
            coin_id,
            TransparentAddressId::new(
                zip32::AccountId::ZERO,
                TransparentScope::External,
                NonHardenedChildIndex::ZERO,
            ),
            String::new(),
            Script::default(),
            Zatoshis::const_from_u64(100_000),
        );
        let funding_transaction = WalletTransaction::new_for_test(
            FUNDING_TXID,
            ConfirmationStatus::Confirmed(FUNDING_HEIGHT),
        )
        .with_transparent_coins_for_test(vec![coin]);
        let spend_scan_target = ScanTarget::internal(SPEND_HEIGHT, spending_record.txid());

        MockWalletBuilder::new()
            .wallet_transactions(HashMap::from([
                (FUNDING_TXID, funding_transaction),
                (spending_record.txid(), spending_record),
            ]))
            .outpoint_map(BTreeMap::from([(coin_id, spend_scan_target)]))
            .create_mock_wallet()
    }

    fn scanned_blocks() -> BTreeMap<BlockHeight, WalletBlock> {
        BTreeMap::from([(
            SPEND_HEIGHT,
            WalletBlock::from_parts(
                SPEND_HEIGHT,
                BlockHash([0; 32]),
                BlockHash([0; 32]),
                0,
                Vec::new(),
                TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 0,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            ),
        )])
    }

    /// Answers transaction requests with `transaction` mined at `SPEND_HEIGHT`.
    fn spawn_fetcher(transaction: &WalletTransaction) -> mpsc::UnboundedSender<FetchRequest> {
        let mut data = Vec::new();
        transaction.transaction().write(&mut data).unwrap();
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::Transaction(reply_sender, _txid) => {
                        let _ignore_error = reply_sender.send(Ok(RawTransaction {
                            data: data.clone(),
                            height: u64::from(SPEND_HEIGHT),
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    fn get_coin_spending_txid(wallet: &MockWallet) -> Option<TxId> {
        wallet
            .get_wallet_transactions()
            .unwrap()
            .get(&FUNDING_TXID)
            .unwrap()
            .transparent_coins()
            .first()
            .unwrap()
            .spending_transaction()
    }

    /// Locates the transparent spends of the wallet's coins and records them in the wallet, as scan results with
    /// no transactions or outpoints of their own are processed.
    async fn update_transparent_spends(
        wallet: &mut MockWallet,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ) -> Result<(), SyncError<MockWalletError>> {
        let mut scanned_transactions = HashMap::new();
        let transparent_spend_scan_targets = spend::locate_transparent_spends(
            &NETWORK,
            &*wallet,
            fetch_request_sender,
            &HashMap::new(),
            &scanned_blocks(),
            &mut scanned_transactions,
            &BTreeMap::new(),
        )
        .await?;
        wallet
            .extend_wallet_transactions(scanned_transactions)
            .unwrap();
        spend::apply_transparent_spends(wallet, transparent_spend_scan_targets).unwrap();

        Ok(())
    }

    #[tokio::test]
    async fn pending_spending_transaction_is_fetched_and_confirmed() {
        let spending_record = spending_record(ConfirmationStatus::Mempool(SPEND_HEIGHT));
        let spending_txid = spending_record.txid();
        let fetch_request_sender = spawn_fetcher(&spending_record);
        let mut wallet = wallet_with_mapped_spend(spending_record);

        update_transparent_spends(&mut wallet, fetch_request_sender)
            .await
            .unwrap();

        assert_eq!(
            wallet
                .get_wallet_transactions()
                .unwrap()
                .get(&spending_txid)
                .unwrap()
                .status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );
        assert_eq!(get_coin_spending_txid(&wallet), Some(spending_txid));
        // the spend stays mapped until cleanup prunes it behind the fully scanned height
        assert!(
            wallet
                .get_outpoints()
                .unwrap()
                .contains_key(&OutputId::new(FUNDING_TXID, 0))
        );
    }

    /// A spending transaction the scanner already confirmed is marked on the coin without fetching the transaction
    /// from the server again.
    #[tokio::test]
    async fn confirmed_spending_transaction_is_not_fetched() {
        let spending_record = spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT));
        let spending_txid = spending_record.txid();
        let mut wallet = wallet_with_mapped_spend(spending_record);
        let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
        // drop receiver so the test fails if the wallet attempts to fetch the transaction again unnecessarily
        drop(fetch_request_receiver);

        update_transparent_spends(&mut wallet, fetch_request_sender)
            .await
            .unwrap();

        assert_eq!(get_coin_spending_txid(&wallet), Some(spending_txid));
    }

    /// The request for the spending transaction fails. The wallet's outpoint map keeps the spend, so the next
    /// sync session detects it again, and the coin is not yet marked as spent.
    #[tokio::test]
    async fn failed_fetch_keeps_the_mapped_spend() {
        let spending_record = spending_record(ConfirmationStatus::Mempool(SPEND_HEIGHT));
        let mut wallet = wallet_with_mapped_spend(spending_record);
        let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
        drop(fetch_request_receiver);

        let result = update_transparent_spends(&mut wallet, fetch_request_sender).await;

        assert!(result.is_err());
        assert!(
            wallet
                .get_outpoints_mut()
                .unwrap()
                .contains_key(&OutputId::new(FUNDING_TXID, 0)),
            "the spend must stay mapped until it is recorded on the coin"
        );
        assert_eq!(get_coin_spending_txid(&wallet), None);
    }
}

mod checked_height_validation {
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::local_consensus::LocalNetwork;
    /// Sapling activates above the first block, so a birthday can fall below it.
    const LOCAL_NETWORK: LocalNetwork = LocalNetwork {
        overwinter: Some(BlockHeight::from_u32(1)),
        ..super::activated_at(3)
    };
    use crate::{error::SyncError, mocks::MockWalletError, sync::checked_wallet_height};
    // It's possible an error from an implementor's get_sync_state could bubble up to checked_wallet_height
    // this test shows that such an error is raies wrapped in a WalletError and return as the Err variant
    #[tokio::test]
    async fn get_sync_state_error() {
        let builder = crate::mocks::MockWalletBuilder::new();
        let test_error = "get_sync_state_error";
        let mut test_wallet = builder
            .get_sync_state_patch(Box::new(|_| {
                Err(MockWalletError::AnErrorVariant(test_error.to_string()))
            }))
            .create_mock_wallet();
        let res = checked_wallet_height(&mut test_wallet, BlockHeight::from_u32(1), &LOCAL_NETWORK);
        assert!(matches!(
            res,
            Err(SyncError::WalletError(
                crate::mocks::MockWalletError::AnErrorVariant(ref s)
            )) if s == test_error
        ));
    }

    mod last_known_chain_height {
        use crate::engine::SyncStateOps;
        use crate::{
            sync::{MAX_REORG_ALLOWANCE, ScanRange},
            wallet::{SyncState, traits::SyncWallet as _},
        };
        const DEFAULT_START_HEIGHT: BlockHeight = BlockHeight::from_u32(1);
        const _DEFAULT_LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(102);
        const DEFAULT_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(110);

        use super::*;
        #[tokio::test]
        async fn above_allowance() {
            // the scan range ends one above the last known chain height, which sits the whole allowance
            // above the chain height
            let last_known_height = DEFAULT_CHAIN_HEIGHT + MAX_REORG_ALLOWANCE + 1;
            let lkch = vec![ScanRange::from_parts(
                DEFAULT_START_HEIGHT..last_known_height,
                crate::sync::ScanPriority::Scanned,
            )];
            let state = SyncState::new_for_test(lkch);
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut test_wallet = builder.sync_state(state).create_mock_wallet();
            let res = checked_wallet_height(&mut test_wallet, DEFAULT_CHAIN_HEIGHT, &LOCAL_NETWORK);
            if let Err(e) = res {
                assert_eq!(
                    e.to_string(),
                    format!(
                        "wallet height {} is more than {} blocks ahead of best chain height {}",
                        last_known_height - 1,
                        MAX_REORG_ALLOWANCE,
                        DEFAULT_CHAIN_HEIGHT
                    )
                );
            } else {
                panic!()
            }
        }
        #[tokio::test]
        async fn above_chain_height_below_allowance() {
            // The hain_height is received from the proxy
            // truncate uses the wallet scan start height
            // as a
            let lkch = vec![ScanRange::from_parts(
                BlockHeight::from_u32(6)..BlockHeight::from_u32(10),
                crate::sync::ScanPriority::Scanned,
            )];
            let state = SyncState::new_for_test(lkch);
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut test_wallet = builder.sync_state(state).create_mock_wallet();
            let chain_height = BlockHeight::from_u32(4);
            // This will trigger a call to truncate_wallet_data with
            // chain_height and start_height inferred from the wallet.
            // chain must be greater than by this time which hits the Greater cmp
            // match
            let res = checked_wallet_height(&mut test_wallet, chain_height, &LOCAL_NETWORK);
            assert_eq!(res.unwrap(), BlockHeight::from_u32(4));
        }
        /// Blocks above the chain height are truncated and scanned again when the chain extends. Transparent
        /// address discovery only covers the blocks at or below the transparent scan floor, and is not performed
        /// again during the sync session, so a floor above the chain height is lowered to it for the compact block
        /// transparent data of the truncated blocks to be scanned. A floor at or below the chain height still
        /// covers every block the wallet keeps.
        #[tokio::test]
        async fn above_chain_height_lowers_transparent_scan_floor() {
            const LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(110);
            const CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(105);
            const FLOOR_ABOVE_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(108);
            const FLOOR_BELOW_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(100);

            for (floor, expected_floor) in [
                (FLOOR_ABOVE_CHAIN_HEIGHT, CHAIN_HEIGHT),
                (CHAIN_HEIGHT, CHAIN_HEIGHT),
                (FLOOR_BELOW_CHAIN_HEIGHT, FLOOR_BELOW_CHAIN_HEIGHT),
            ] {
                let mut state = SyncState::new_for_test(vec![ScanRange::from_parts(
                    DEFAULT_START_HEIGHT..LAST_KNOWN_HEIGHT + 1,
                    crate::sync::ScanPriority::Scanned,
                )]);
                state.set_transparent_scan_floor(floor);
                let mut test_wallet = crate::mocks::MockWalletBuilder::new()
                    .sync_state(state)
                    .create_mock_wallet();

                let last_known_chain_height =
                    checked_wallet_height(&mut test_wallet, CHAIN_HEIGHT, &LOCAL_NETWORK).unwrap();

                assert_eq!(last_known_chain_height, CHAIN_HEIGHT);
                assert_eq!(
                    test_wallet
                        .get_sync_state()
                        .expect("the mock wallet holds a sync state")
                        .transparent_scan_floor(),
                    Some(expected_floor),
                    "floor {floor}"
                );
            }
        }
        #[ignore = "in progress"]
        #[tokio::test]
        async fn equal_or_below_chain_height_and_above_sapling() {
            let lkch = vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                crate::sync::ScanPriority::Scanned,
            )];
            let state = SyncState::new_for_test(lkch);
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
        }
        #[ignore = "in progress"]
        #[tokio::test]
        async fn equal_or_below_chain_height_and_below_sapling() {
            // This case requires that the wallet have a scan_start_below sapling
            // which is an unexpected state.
            let lkch = vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                crate::sync::ScanPriority::Scanned,
            )];
            let state = SyncState::new_for_test(lkch);
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
        }
        #[ignore = "in progress"]
        #[tokio::test]
        async fn below_sapling() {
            let lkch = vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                crate::sync::ScanPriority::Scanned,
            )];
            let state = SyncState::new_for_test(lkch);
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
        }
    }
    mod no_last_known_chain_height {
        use super::*;
        // If there are know scan_ranges in the SyncState
        #[tokio::test]
        async fn get_bday_error() {
            let test_error = "get_bday_error";
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut test_wallet = builder
                .get_birthday_patch(Box::new(|_| {
                    Err(crate::mocks::MockWalletError::AnErrorVariant(
                        test_error.to_string(),
                    ))
                }))
                .create_mock_wallet();
            let res =
                checked_wallet_height(&mut test_wallet, BlockHeight::from_u32(1), &LOCAL_NETWORK);
            assert!(matches!(
                res,
                Err(SyncError::WalletError(
                    crate::mocks::MockWalletError::AnErrorVariant(ref s)
                )) if s == test_error
            ));
        }
        #[ignore = "in progress"]
        #[tokio::test]
        async fn raw_bday_above_chain_height() {
            let builder = crate::mocks::MockWalletBuilder::new();
            let mut test_wallet = builder
                .birthday(BlockHeight::from_u32(15))
                .create_mock_wallet();
            let res =
                checked_wallet_height(&mut test_wallet, BlockHeight::from_u32(1), &LOCAL_NETWORK);
            if let Err(e) = res {
                assert_eq!(
                    e.to_string(),
                    format!(
                        "wallet height is more than {} blocks ahead of best chain height",
                        15 - 1
                    )
                );
            } else {
                panic!()
            }
        }
        mod sapling_height {
            use super::*;
            #[tokio::test]
            async fn raw_bday_above() {
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .birthday(BlockHeight::from_u32(4))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(5),
                    &LOCAL_NETWORK,
                );
                assert_eq!(res.unwrap(), BlockHeight::from_u32(4 - 1));
            }
            #[tokio::test]
            async fn raw_bday_equal() {
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .birthday(BlockHeight::from_u32(3))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(5),
                    &LOCAL_NETWORK,
                );
                assert_eq!(res.unwrap(), BlockHeight::from_u32(3 - 1));
            }
            #[tokio::test]
            async fn raw_bday_below() {
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .birthday(BlockHeight::from_u32(1))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(5),
                    &LOCAL_NETWORK,
                );
                assert!(matches!(res, Err(SyncError::BirthdayBelowSapling(1, 3))));
            }
        }
    }
}

mod expire_transactions {
    use std::collections::HashMap;

    use zcash_protocol::TxId;
    use zcash_protocol::consensus::BlockHeight;
    use zl_common::status::ConfirmationStatus;

    use crate::mocks::{MockWallet, MockWalletBuilder};
    use crate::sync::{ScanPriority, ScanRange, expire_transactions};
    use crate::wallet::{SyncState, WalletTransaction};

    const UNSCANNED_START: u32 = 51;
    const UNSCANNED_END: u32 = 91;
    const CHAIN_HEIGHT: u32 = 200;

    /// Creates a mock wallet with all blocks scanned up to `chain_height`.
    fn wallet_at_height(chain_height: u32, transactions: Vec<WalletTransaction>) -> MockWallet {
        wallet_with_scan_ranges(
            vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(chain_height + 1),
                ScanPriority::Scanned,
            )],
            transactions,
        )
    }

    /// Creates a mock wallet whose blocks up to [`CHAIN_HEIGHT`] are scanned, except for the range from
    /// [`UNSCANNED_START`] to [`UNSCANNED_END`].
    fn wallet_with_unscanned_range(transactions: Vec<WalletTransaction>) -> MockWallet {
        wallet_with_scan_ranges(
            vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(UNSCANNED_START),
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(UNSCANNED_START)..BlockHeight::from_u32(UNSCANNED_END),
                    ScanPriority::Historic,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(UNSCANNED_END)..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                    ScanPriority::Scanned,
                ),
            ],
            transactions,
        )
    }

    fn wallet_with_scan_ranges(
        scan_ranges: Vec<ScanRange>,
        transactions: Vec<WalletTransaction>,
    ) -> MockWallet {
        let sync_state = SyncState::new_for_test(scan_ranges);
        let wallet_transactions: HashMap<TxId, WalletTransaction> = transactions
            .into_iter()
            .map(|transaction| (transaction.txid(), transaction))
            .collect();

        MockWalletBuilder::new()
            .sync_state(sync_state)
            .wallet_transactions(wallet_transactions)
            .create_mock_wallet()
    }

    fn transaction_status(wallet: &MockWallet, txid: TxId) -> ConfirmationStatus {
        crate::wallet::traits::SyncTransactions::get_wallet_transactions(wallet)
            .unwrap()
            .get(&txid)
            .unwrap()
            .status()
    }

    #[test]
    fn pending_transaction_past_expiry_is_failed() {
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
            BlockHeight::from_u32(100),
        );
        let mut wallet = wallet_at_height(100, vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Failed(_)
        ));
    }

    #[test]
    fn pending_transaction_before_expiry_is_untouched() {
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
            BlockHeight::from_u32(101),
        );
        let mut wallet = wallet_at_height(100, vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Mempool(_)
        ));
    }

    #[test]
    fn zero_expiry_transaction_never_expires() {
        // ZIP-203: an expiry height of 0 means the transaction never expires.
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
            BlockHeight::from_u32(0),
        );
        let mut wallet = wallet_at_height(1_000_000, vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Mempool(_)
        ));
    }

    #[test]
    fn pending_transaction_with_expiry_above_an_unscanned_range_is_untouched() {
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Mempool(BlockHeight::from_u32(UNSCANNED_START - 1)),
            BlockHeight::from_u32(UNSCANNED_END),
        );
        let mut wallet = wallet_with_unscanned_range(vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Mempool(_)
        ));
    }

    #[test]
    fn pending_transaction_with_expiry_below_an_unscanned_range_is_failed() {
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Mempool(BlockHeight::from_u32(1)),
            BlockHeight::from_u32(UNSCANNED_START - 1),
        );
        let mut wallet = wallet_with_unscanned_range(vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Failed(_)
        ));
    }

    #[test]
    fn confirmed_transaction_is_untouched() {
        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_expiry(
            txid,
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(61)),
            BlockHeight::from_u32(100),
        );
        let mut wallet = wallet_at_height(100, vec![transaction]);

        expire_transactions(&mut wallet).unwrap();

        assert!(matches!(
            transaction_status(&wallet, txid),
            ConfirmationStatus::Confirmed(_)
        ));
    }
}

/// Stale scan results, whether a scan succeeded or failed, are discarded before they are processed.
mod stale_scan_results {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use tokio::sync::mpsc;
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;
    use zcash_transparent::keys::NonHardenedChildIndex;

    use crate::{
        config::PerformanceLevel,
        error::{ContinuityError, ScanError},
        keys::transparent::{TransparentAddressId, TransparentScope},
        mocks::{MockWallet, MockWalletBuilder},
        scan::{
            ScanResults,
            task::{ScanLoad, TaskId},
        },
        sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
        wallet::{NullifierMap, SyncState, traits::SyncWallet as _},
    };

    use super::NETWORK;
    const BIRTHDAY: u32 = 1;
    /// The chain height the server reported when the scan range was selected.
    const CHAIN_HEIGHT: u32 = 40;

    fn wallet_with_scan_ranges(scan_ranges: Vec<ScanRange>) -> MockWallet {
        MockWalletBuilder::new()
            .sync_state(SyncState::new_for_test(scan_ranges))
            .create_mock_wallet()
    }

    async fn process(
        wallet: &mut MockWallet,
        scan_range: ScanRange,
        scan_results: Result<ScanResults, ScanError>,
    ) -> ProcessedScanResults {
        let (fetch_request_sender, _) = mpsc::unbounded_channel();
        let task_id = TaskId::first();
        let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);
        let previous_chain_height = scan_range.block_range().end - 1;

        process_scan_results(
            &NETWORK,
            wallet,
            fetch_request_sender,
            &HashMap::new(),
            ScanLoad {
                task_id,
                chain_height: scan_range.block_range().end - 1,
                highest_scanned_height: scan_range.block_range().start - 1,
                scan_range,
            },
            &in_flight_tasks,
            scan_results,
            previous_chain_height,
            PerformanceLevel::High,
            &mut false,
        )
        .await
        .expect("stale scan results are discarded")
    }

    /// The chain height dropped by one block while the wallet's whole range was being scanned. The scan results
    /// hold a block that has left the chain, so they are discarded with the transparent gap addresses derived
    /// from them, and the range the wallet still holds is set back to the priority it was selected with.
    #[tokio::test]
    async fn truncated_scan_range_is_reset_and_its_scan_results_discarded() {
        let mut wallet = wallet_with_scan_ranges(vec![ScanRange::from_parts(
            BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
            ScanPriority::Scanning,
        )]);
        let gap_address = (
            "gap address".to_string(),
            TransparentAddressId::new(
                zip32::AccountId::ZERO,
                TransparentScope::External,
                NonHardenedChildIndex::ZERO,
            ),
        );

        let processed = process(
            &mut wallet,
            ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                ScanPriority::ChainTip,
            ),
            Ok(ScanResults {
                nullifiers: NullifierMap::new(),
                outpoints: BTreeMap::new(),
                scanned_blocks: BTreeMap::new(),
                dropped_scan_targets: BTreeSet::new(),
                wallet_transactions: HashMap::new(),
                sapling_located_trees: Vec::new(),
                orchard_located_trees: Vec::new(),
                ironwood_located_trees: Vec::new(),
                new_transparent_inuse_addresses: HashMap::from([gap_address.clone()]),
                new_transparent_gap_addresses: HashMap::from([gap_address]),
            }),
        )
        .await;

        assert!(processed.new_transparent_inuse_addresses.is_empty());
        assert!(processed.new_transparent_gap_addresses.is_empty());
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            [ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
                ScanPriority::ChainTip,
            )]
        );
    }

    /// The chain height dropped below a newly mined block while it was being verified, so the wallet holds none
    /// of its scan range. Its scan failed the continuity check, which is handled as a re-org of the scan range.
    /// The error is discarded as the scan range it would reset has left the wallet.
    #[tokio::test]
    async fn error_of_a_removed_scan_range_is_discarded() {
        let scan_ranges = vec![ScanRange::from_parts(
            BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
            ScanPriority::Scanned,
        )];
        let mut wallet = wallet_with_scan_ranges(scan_ranges.clone());

        process(
            &mut wallet,
            ScanRange::from_parts(
                BlockHeight::from_u32(CHAIN_HEIGHT)..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                ScanPriority::Verify,
            ),
            Err(ScanError::ContinuityError(
                ContinuityError::HashDiscontinuity {
                    height: BlockHeight::from_u32(CHAIN_HEIGHT),
                    prev_hash: BlockHash([1; 32]),
                    previous_block_hash: BlockHash([2; 32]),
                },
            )),
        )
        .await;

        assert_eq!(wallet.get_sync_state().unwrap().scan_ranges(), scan_ranges);
    }
}

/// The checkpoints [`crate::wallet::traits::SyncShardTrees::update_shard_trees`] adds for the blocks of a scan
/// range in a pool that has no note commitments in these blocks. The pool's commitment tree is unchanged as of
/// such a block, so its checkpoint copies the tree state of the checkpoint of the block below, which is
/// fetched from the server where the shard tree is missing it.
mod shard_tree_checkpoints {
    use std::sync::Arc;
    use std::sync::atomic::{self, AtomicUsize};

    use incrementalmerkletree::frontier::CommitmentTree;
    use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
    use orchard::tree::MerkleHashOrchard;
    use shardtree::store::{self, ShardStore as _};
    use tokio::sync::mpsc;
    use zcash_primitives::merkle_tree::write_commitment_tree;
    use zcash_protocol::consensus::BlockHeight;
    use zl_netutils::lightwallet_protocol::TreeState;

    use crate::{
        client::FetchRequest,
        error::SyncError,
        mocks::{MockWallet, MockWalletBuilder, MockWalletError},
        shardtree_ext::ShardTreeExt,
        sync::{SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE, ScanPriority, ScanRange},
        wallet::{ShardTrees, traits::SyncShardTrees},
    };

    use super::NETWORK;
    const HIGHEST_SCANNED: u32 = 10;
    /// A chain height whose rolling window lies above the scanned blocks.
    const CHAIN_HEIGHT_ABOVE_THE_WINDOW: u32 =
        SECOND_SCANNED_BLOCK + 2 * SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;
    const FIRST_SCANNED_BLOCK: u32 = HIGHEST_SCANNED + 1;
    const SECOND_SCANNED_BLOCK: u32 = FIRST_SCANNED_BLOCK + 1;
    const SHIELDED_POOLS: usize = 3;
    /// The orchard note commitments the chain holds as of the scanned blocks.
    const ORCHARD_TREE_SIZE: u64 = 3;

    /// Updates the wallet's shard trees with the scan of the two blocks above the highest scanned height, which
    /// hold no note commitments of any pool. The chain height is the second block.
    async fn update_shard_trees(
        wallet: &mut MockWallet,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ) -> Result<(), SyncError<MockWalletError>> {
        wallet
            .update_shard_trees(
                &NETWORK,
                fetch_request_sender,
                &ScanRange::from_parts(
                    BlockHeight::from_u32(FIRST_SCANNED_BLOCK)
                        ..BlockHeight::from_u32(SECOND_SCANNED_BLOCK + 1),
                    ScanPriority::ChainTip,
                ),
                BlockHeight::from_u32(SECOND_SCANNED_BLOCK),
                BlockHeight::from_u32(HIGHEST_SCANNED),
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .await
    }

    fn wallet(shard_trees: ShardTrees) -> MockWallet {
        MockWalletBuilder::new()
            .shard_trees(shard_trees)
            .create_mock_wallet()
    }

    /// A fetcher whose every request fails.
    fn failing_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
        let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
        drop(fetch_request_receiver);

        fetch_request_sender
    }

    /// Answers tree state requests with an orchard commitment tree of `ORCHARD_TREE_SIZE` note commitments and
    /// empty sapling and ironwood commitment trees, counting the requests.
    fn spawn_counting_fetcher() -> (mpsc::UnboundedSender<FetchRequest>, Arc<AtomicUsize>) {
        const EMPTY_TREE: &str = "000000";
        let mut orchard_tree = CommitmentTree::<
            MerkleHashOrchard,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        >::empty();
        for _ in 0..ORCHARD_TREE_SIZE {
            orchard_tree
                .append(MerkleHashOrchard::empty_leaf())
                .unwrap();
        }
        let mut orchard_tree_bytes = Vec::new();
        write_commitment_tree(&orchard_tree, &mut orchard_tree_bytes).unwrap();
        let orchard_tree = hex::encode(orchard_tree_bytes);

        let request_count = Arc::new(AtomicUsize::new(0));
        let counted_requests = request_count.clone();
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        counted_requests.fetch_add(1, atomic::Ordering::Release);
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: EMPTY_TREE.to_string(),
                            orchard_tree: orchard_tree.clone(),
                            ironwood_tree: EMPTY_TREE.to_string(),
                            ..Default::default()
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        (fetch_request_sender, request_count)
    }

    /// The tree states of the sapling, orchard and ironwood checkpoints at `height`.
    fn checkpoint_tree_states(
        wallet: &mut MockWallet,
        height: u32,
    ) -> [Option<store::TreeState>; SHIELDED_POOLS] {
        let height = BlockHeight::from_u32(height);
        let shard_trees = wallet.get_shard_trees_mut().unwrap();
        [
            shard_trees.sapling.store().get_checkpoint(&height).unwrap(),
            shard_trees.orchard.store().get_checkpoint(&height).unwrap(),
            shard_trees
                .ironwood
                .store()
                .get_checkpoint(&height)
                .unwrap(),
        ]
        .map(|checkpoint| checkpoint.map(|checkpoint| checkpoint.tree_state()))
    }

    /// A load below the chain's rolling window places a checkpoint on its last block and on no other, in every
    /// pool. The block below holds no checkpoint, so each pool fetches the tree state once, and a session
    /// stopped after the load anchors a spend there.
    #[tokio::test]
    async fn a_load_below_the_window_checkpoints_its_last_block_in_every_pool() {
        let mut wallet = wallet(ShardTrees::new());
        let (fetch_request_sender, request_count) = spawn_counting_fetcher();
        wallet
            .update_shard_trees(
                &NETWORK,
                fetch_request_sender,
                &ScanRange::from_parts(
                    BlockHeight::from_u32(FIRST_SCANNED_BLOCK)
                        ..BlockHeight::from_u32(SECOND_SCANNED_BLOCK + 1),
                    ScanPriority::Historic,
                ),
                BlockHeight::from_u32(CHAIN_HEIGHT_ABOVE_THE_WINDOW),
                BlockHeight::from_u32(HIGHEST_SCANNED),
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
            [None, None, None]
        );
        assert_eq!(
            checkpoint_tree_states(&mut wallet, SECOND_SCANNED_BLOCK),
            [
                Some(store::TreeState::Empty),
                Some(store::TreeState::AtPosition(Position::from(
                    ORCHARD_TREE_SIZE - 1,
                ))),
                Some(store::TreeState::Empty),
            ]
        );
        assert_eq!(
            request_count.load(atomic::Ordering::Acquire),
            SHIELDED_POOLS
        );
    }

    /// Every shard tree holds the checkpoint of the highest scanned block, so the checkpoints of the scanned
    /// blocks copy its tree state with no request to the server.
    #[tokio::test]
    async fn checkpoints_copy_the_block_below() {
        let mut shard_trees = ShardTrees::new();
        let highest_scanned_height = BlockHeight::from_u32(HIGHEST_SCANNED);
        shard_trees
            .sapling
            .append_checkpoint(highest_scanned_height)
            .unwrap();
        shard_trees
            .orchard
            .append(
                MerkleHashOrchard::empty_leaf(),
                Retention::Checkpoint {
                    id: highest_scanned_height,
                    marking: Marking::None,
                },
            )
            .unwrap();
        shard_trees
            .ironwood
            .append_checkpoint(highest_scanned_height)
            .unwrap();
        let mut wallet = wallet(shard_trees);

        update_shard_trees(&mut wallet, failing_fetcher())
            .await
            .unwrap();

        let highest_scanned_tree_states = [
            Some(store::TreeState::Empty),
            Some(store::TreeState::AtPosition(Position::from(0))),
            Some(store::TreeState::Empty),
        ];
        assert_eq!(
            checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
            highest_scanned_tree_states
        );
        assert_eq!(
            checkpoint_tree_states(&mut wallet, SECOND_SCANNED_BLOCK),
            highest_scanned_tree_states
        );
    }

    /// The shard trees are missing the checkpoint of the highest scanned block, so the tree state of the first
    /// scanned block is fetched for each pool. The checkpoint of the second scanned block copies it with no
    /// further request.
    #[tokio::test]
    async fn missing_checkpoint_below_is_fetched_once_for_each_pool() {
        let mut wallet = wallet(ShardTrees::new());
        let (fetch_request_sender, request_count) = spawn_counting_fetcher();

        update_shard_trees(&mut wallet, fetch_request_sender)
            .await
            .unwrap();

        let fetched_tree_states = [
            Some(store::TreeState::Empty),
            Some(store::TreeState::AtPosition(Position::from(
                ORCHARD_TREE_SIZE - 1,
            ))),
            Some(store::TreeState::Empty),
        ];
        assert_eq!(
            checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
            fetched_tree_states
        );
        assert_eq!(
            checkpoint_tree_states(&mut wallet, SECOND_SCANNED_BLOCK),
            fetched_tree_states
        );
        assert_eq!(
            request_count.load(atomic::Ordering::Acquire),
            SHIELDED_POOLS
        );
    }

    /// The sapling tree holds the checkpoint of the highest scanned block and the orchard tree is missing it.
    /// The request for the orchard tree state fails, so the sapling checkpoint that was determined before the
    /// request is left out of the sapling tree.
    #[tokio::test]
    async fn failed_request_leaves_the_shard_trees_as_they_were() {
        let mut shard_trees = ShardTrees::new();
        shard_trees
            .sapling
            .append_checkpoint(BlockHeight::from_u32(HIGHEST_SCANNED))
            .unwrap();
        let mut wallet = wallet(shard_trees);

        let result = update_shard_trees(&mut wallet, failing_fetcher()).await;

        assert!(result.is_err());
        assert_eq!(
            checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
            [None; SHIELDED_POOLS]
        );
    }
}

/// Scan results are added to the wallet together, once every server request the update depends on is answered.
mod scan_results_wallet_update {
    use crate::engine::{ScanTargetOps, WalletBlockOps};
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
    use orchard::tree::MerkleHashOrchard;
    use shardtree::store::ShardStore as _;
    use tokio::sync::mpsc;
    use zcash_primitives::{block::BlockHash, transaction::TxId};
    use zcash_protocol::consensus::BlockHeight;
    use zl_common::status::ConfirmationStatus;
    use zl_netutils::lightwallet_protocol::{RawTransaction, TreeState};

    use super::spend_reset_lifecycle::{
        FUNDING_HEIGHT, FUNDING_TXID, NOTE_NULLIFIER, SPEND_HEIGHT, funding_transaction,
        spending_key,
    };
    use crate::{
        client::FetchRequest,
        config::PerformanceLevel,
        error::SyncError,
        mocks::{MockWallet, MockWalletBuilder, MockWalletError},
        scan::{
            ScanResults,
            task::{ScanLoad, TaskId},
        },
        sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
        wallet::{
            NullifierMap, OutputId, OutputInterface as _, ScanTarget, SyncState, TreeBounds,
            WalletBlock, WalletTransaction,
            traits::{
                SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions,
                SyncWallet,
            },
        },
        witness,
    };

    use super::NETWORK;
    const BIRTHDAY: u32 = 1;
    /// A transaction of the wallet that the scan of the block at `SPEND_HEIGHT` found by trial decryption.
    const SCANNED_TXID: TxId = TxId::from_bytes([3; 32]);

    /// The transaction that spends the wallet's note with no change, under the txid the server returns it with.
    fn spending_transaction() -> WalletTransaction {
        let status = ConfirmationStatus::Confirmed(SPEND_HEIGHT);
        let txid = WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
            .transaction()
            .txid();
        WalletTransaction::new_for_test(txid, status)
    }

    /// Answers tree state requests with empty note commitment trees, and transaction requests with
    /// `spending_transaction` mined at `SPEND_HEIGHT`. With no spending transaction, transaction requests fail.
    fn spawn_fetcher(
        spending_transaction: Option<&WalletTransaction>,
    ) -> mpsc::UnboundedSender<FetchRequest> {
        const EMPTY_TREE: &str = "000000";
        let spending_transaction_data = spending_transaction.map(|spending_transaction| {
            let mut data = Vec::new();
            spending_transaction.transaction().write(&mut data).unwrap();
            data
        });
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: EMPTY_TREE.to_string(),
                            orchard_tree: EMPTY_TREE.to_string(),
                            ironwood_tree: EMPTY_TREE.to_string(),
                            ..Default::default()
                        }));
                    }
                    FetchRequest::Transaction(reply_sender, _txid) => {
                        // the request fails when the reply sender is dropped.
                        if let Some(data) = &spending_transaction_data {
                            let _ignore_error = reply_sender.send(Ok(RawTransaction {
                                data: data.clone(),
                                height: u64::from(SPEND_HEIGHT),
                            }));
                        }
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    /// The block at `SPEND_HEIGHT`, which holds the first orchard note commitment of the chain.
    fn block_at_spend_height() -> WalletBlock {
        WalletBlock::from_parts(
            SPEND_HEIGHT,
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 1,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
        )
    }

    /// The scan ranges of a wallet scanned up to `SPEND_HEIGHT`, with the block at `SPEND_HEIGHT` in the given
    /// priority.
    fn scan_ranges(spend_height_priority: ScanPriority) -> Vec<ScanRange> {
        vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..SPEND_HEIGHT,
                ScanPriority::Scanned,
            ),
            ScanRange::from_parts(SPEND_HEIGHT..SPEND_HEIGHT + 1, spend_height_priority),
        ]
    }

    /// The scan ranges once the block at `SPEND_HEIGHT` is scanned.
    fn scanned_scan_ranges() -> Vec<ScanRange> {
        vec![ScanRange::from_parts(
            BlockHeight::from_u32(BIRTHDAY)..SPEND_HEIGHT + 1,
            ScanPriority::Scanned,
        )]
    }

    /// A wallet holding one unspent sapling note, with the block at `SPEND_HEIGHT` in the given priority.
    fn wallet_builder(spend_height_priority: ScanPriority) -> MockWalletBuilder {
        MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(BIRTHDAY))
            .sync_state(SyncState::new_for_test(scan_ranges(spend_height_priority)))
            .wallet_transactions(HashMap::from([(FUNDING_TXID, funding_transaction(None))]))
    }

    /// The block at `FUNDING_HEIGHT`, which adds no note commitments.
    fn block_at_funding_height() -> WalletBlock {
        WalletBlock::from_parts(
            FUNDING_HEIGHT,
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
        )
    }

    /// The scan ranges of a wallet scanned up to `SPEND_HEIGHT` except for the block at `FUNDING_HEIGHT`, which
    /// is in the given priority.
    fn funding_scan_ranges(funding_height_priority: ScanPriority) -> Vec<ScanRange> {
        vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..FUNDING_HEIGHT,
                ScanPriority::Scanned,
            ),
            ScanRange::from_parts(FUNDING_HEIGHT..FUNDING_HEIGHT + 1, funding_height_priority),
            ScanRange::from_parts(FUNDING_HEIGHT + 1..SPEND_HEIGHT + 1, ScanPriority::Scanned),
        ]
    }

    /// A wallet that scanned the block at `SPEND_HEIGHT` before the block at `FUNDING_HEIGHT`, so it holds the
    /// nullifier of a note it has not yet received, as mapped from the block at `SPEND_HEIGHT`, where
    /// `spending_txid` spends the note.
    fn wallet_with_mapped_spend(spending_txid: TxId) -> MockWallet {
        MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(BIRTHDAY))
            .sync_state(SyncState::new_for_test(funding_scan_ranges(
                ScanPriority::Scanning,
            )))
            .nullifier_map(mapped_spend(spending_txid))
            .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
            .create_mock_wallet()
    }

    /// The scan results of the block at `FUNDING_HEIGHT`, which holds the transaction funding the wallet's
    /// note and nothing else.
    fn funding_scan_results() -> ScanResults {
        ScanResults {
            nullifiers: NullifierMap::new(),
            outpoints: BTreeMap::new(),
            scanned_blocks: BTreeMap::from([(FUNDING_HEIGHT, block_at_funding_height())]),
            dropped_scan_targets: BTreeSet::new(),
            wallet_transactions: HashMap::from([(FUNDING_TXID, funding_transaction(None))]),
            sapling_located_trees: Vec::new(),
            orchard_located_trees: Vec::new(),
            ironwood_located_trees: Vec::new(),
            new_transparent_inuse_addresses: HashMap::new(),
            new_transparent_gap_addresses: HashMap::new(),
        }
    }

    /// The nullifier of the wallet's note as mapped from the block at `SPEND_HEIGHT`, where `spending_txid`
    /// spends the note.
    fn mapped_spend(spending_txid: TxId) -> NullifierMap {
        let mut nullifiers = NullifierMap::new();
        nullifiers.sapling.insert(
            NOTE_NULLIFIER,
            ScanTarget::internal(SPEND_HEIGHT, spending_txid),
        );

        nullifiers
    }

    /// The scan results of the block at `SPEND_HEIGHT`. The block holds a transaction of the wallet, one
    /// orchard note commitment, a transparent input and the spend of the wallet's note by `spending_txid`,
    /// which paid no change and so is missing from the scanned transactions.
    fn scan_results(spending_txid: TxId) -> ScanResults {
        const LOCATED_TREE_SIZE: usize = 1;
        ScanResults {
            nullifiers: mapped_spend(spending_txid),
            outpoints: BTreeMap::from([(
                OutputId::new(SCANNED_TXID, 0),
                ScanTarget::internal(SPEND_HEIGHT, spending_txid),
            )]),
            scanned_blocks: BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]),
            dropped_scan_targets: BTreeSet::new(),
            wallet_transactions: HashMap::from([(
                SCANNED_TXID,
                WalletTransaction::new_for_test(
                    SCANNED_TXID,
                    ConfirmationStatus::Confirmed(SPEND_HEIGHT),
                ),
            )]),
            sapling_located_trees: Vec::new(),
            orchard_located_trees: witness::build_located_trees(
                Position::from(0),
                vec![(
                    MerkleHashOrchard::empty_leaf(),
                    Retention::Checkpoint {
                        id: SPEND_HEIGHT,
                        marking: Marking::None,
                    },
                )],
                LOCATED_TREE_SIZE,
            ),
            ironwood_located_trees: Vec::new(),
            new_transparent_inuse_addresses: HashMap::new(),
            new_transparent_gap_addresses: HashMap::new(),
        }
    }

    /// Processes the scan results of the block at `SPEND_HEIGHT`, selected for scanning with
    /// `selected_priority`.
    async fn process(
        wallet: &mut MockWallet,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        selected_priority: ScanPriority,
        scan_results: ScanResults,
    ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
        process_block(
            wallet,
            fetch_request_sender,
            SPEND_HEIGHT,
            selected_priority,
            scan_results,
        )
        .await
    }

    /// Processes the scan results of the block at `block_height`, selected for scanning with
    /// `selected_priority`, for the wallet's only account.
    async fn process_block(
        wallet: &mut MockWallet,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        block_height: BlockHeight,
        selected_priority: ScanPriority,
        scan_results: ScanResults,
    ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
        let ufvks = HashMap::from([(
            zip32::AccountId::ZERO,
            spending_key().to_unified_full_viewing_key(),
        )]);
        let task_id = TaskId::first();
        let scan_range = ScanRange::from_parts(block_height..block_height + 1, selected_priority);
        let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);

        process_scan_results(
            &NETWORK,
            wallet,
            fetch_request_sender,
            &ufvks,
            ScanLoad {
                task_id,
                chain_height: scan_range.block_range().end - 1,
                highest_scanned_height: scan_range.block_range().start - 1,
                scan_range,
            },
            &in_flight_tasks,
            Ok(scan_results),
            block_height,
            PerformanceLevel::High,
            &mut false,
        )
        .await
    }

    fn note_spending_txid(wallet: &MockWallet) -> Option<TxId> {
        wallet.get_wallet_transactions().unwrap()[&FUNDING_TXID].sapling_notes()[0]
            .spending_transaction()
    }

    fn orchard_tree_size(wallet: &mut MockWallet) -> Option<Position> {
        wallet
            .get_shard_trees_mut()
            .unwrap()
            .orchard
            .max_leaf_position(None)
            .unwrap()
    }

    /// The scan of the block at `SPEND_HEIGHT` maps the nullifier of the wallet's note, which a transaction with
    /// no change spent. Every part of the scan results is added to the wallet: the scanned transaction, the
    /// note commitment with a checkpoint in each shard tree, the spend with its fetched spending transaction,
    /// the block and the scanned scan range.
    #[tokio::test]
    async fn scan_results_are_added_to_the_wallet() {
        let spending_transaction = spending_transaction();
        let spending_txid = spending_transaction.txid();
        let mut wallet = wallet_builder(ScanPriority::Scanning).create_mock_wallet();

        let result = process(
            &mut wallet,
            spawn_fetcher(Some(&spending_transaction)),
            ScanPriority::ChainTip,
            scan_results(spending_txid),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
        let wallet_transactions = wallet.get_wallet_transactions().unwrap();
        assert!(wallet_transactions.contains_key(&SCANNED_TXID));
        assert_eq!(
            wallet_transactions[&spending_txid].status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );
        assert_eq!(orchard_tree_size(&mut wallet), Some(Position::from(0)));
        let shard_trees = wallet.get_shard_trees_mut().unwrap();
        let sapling_checkpoint = shard_trees
            .sapling
            .store()
            .get_checkpoint(&SPEND_HEIGHT)
            .unwrap();
        let orchard_checkpoint = shard_trees
            .orchard
            .store()
            .get_checkpoint(&SPEND_HEIGHT)
            .unwrap();
        let ironwood_checkpoint = shard_trees
            .ironwood
            .store()
            .get_checkpoint(&SPEND_HEIGHT)
            .unwrap();
        assert!(
            sapling_checkpoint.is_some()
                && orchard_checkpoint.is_some()
                && ironwood_checkpoint.is_some(),
            "every shard tree must hold a checkpoint at the scanned block"
        );
        assert!(wallet.get_wallet_block(SPEND_HEIGHT).is_ok());
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            scanned_scan_ranges()
        );
        // the wallet is fully scanned, so the mapped nullifiers and outpoints are all removed.
        assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
        assert!(wallet.get_outpoints().unwrap().is_empty());
    }

    /// The request for the spending transaction fails, so the wallet is left as it was before the scan results
    /// were processed. The scan range is scanned again in the next sync session, where the spend is detected
    /// again.
    #[tokio::test]
    async fn failed_spending_transaction_request_leaves_the_wallet_as_it_was() {
        let mut wallet = wallet_builder(ScanPriority::Scanning).create_mock_wallet();

        let result = process(
            &mut wallet,
            spawn_fetcher(None),
            ScanPriority::ChainTip,
            scan_results(spending_transaction().txid()),
        )
        .await;

        assert!(result.is_err());
        assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
        assert!(wallet.get_outpoints().unwrap().is_empty());
        assert_eq!(orchard_tree_size(&mut wallet), None);
        assert!(wallet.get_wallet_block(SPEND_HEIGHT).is_err());
        assert_eq!(wallet.get_wallet_transactions().unwrap().len(), 1);
        assert_eq!(note_spending_txid(&wallet), None);
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            scan_ranges(ScanPriority::Scanning)
        );
    }

    /// The block at `SPEND_HEIGHT` was scanned with its nullifiers discarded, and the nullifiers were then
    /// fetched again. The spend they map is recorded on the wallet's note with its fetched spending
    /// transaction, and the scan range is scanned. The shard trees already hold the block and are left as they
    /// are.
    #[tokio::test]
    async fn refetched_nullifiers_record_the_spend() {
        let spending_transaction = spending_transaction();
        let spending_txid = spending_transaction.txid();
        let mut wallet = wallet_builder(ScanPriority::RefetchingNullifiers)
            .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
            .create_mock_wallet();
        let scan_results = ScanResults {
            nullifiers: mapped_spend(spending_txid),
            outpoints: BTreeMap::new(),
            scanned_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            dropped_scan_targets: BTreeSet::new(),
            sapling_located_trees: Vec::new(),
            orchard_located_trees: Vec::new(),
            ironwood_located_trees: Vec::new(),
            new_transparent_inuse_addresses: HashMap::new(),
            new_transparent_gap_addresses: HashMap::new(),
        };

        let result = process(
            &mut wallet,
            spawn_fetcher(Some(&spending_transaction)),
            ScanPriority::ScannedWithoutMapping,
            scan_results,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
        assert_eq!(
            wallet.get_wallet_transactions().unwrap()[&spending_txid].status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );
        assert_eq!(orchard_tree_size(&mut wallet), None);
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            scanned_scan_ranges()
        );
    }

    /// The request for the spending transaction fails while the re-fetched nullifiers are processed, so the
    /// wallet is left as it was and the nullifiers are fetched again in the next sync session.
    #[tokio::test]
    async fn failed_request_for_refetched_nullifiers_leaves_the_wallet_as_it_was() {
        let mut wallet = wallet_builder(ScanPriority::RefetchingNullifiers)
            .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
            .create_mock_wallet();
        let scan_results = ScanResults {
            nullifiers: mapped_spend(spending_transaction().txid()),
            outpoints: BTreeMap::new(),
            scanned_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            dropped_scan_targets: BTreeSet::new(),
            sapling_located_trees: Vec::new(),
            orchard_located_trees: Vec::new(),
            ironwood_located_trees: Vec::new(),
            new_transparent_inuse_addresses: HashMap::new(),
            new_transparent_gap_addresses: HashMap::new(),
        };

        let result = process(
            &mut wallet,
            spawn_fetcher(None),
            ScanPriority::ScannedWithoutMapping,
            scan_results,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(wallet.get_wallet_transactions().unwrap().len(), 1);
        assert_eq!(note_spending_txid(&wallet), None);
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            scan_ranges(ScanPriority::RefetchingNullifiers)
        );
    }

    /// The block at `SPEND_HEIGHT` was scanned before the block at `FUNDING_HEIGHT`, so the wallet's nullifier
    /// map holds the spend of a note the wallet has not yet received. Scanning the funding block adds the note
    /// and locates its spend in the wallet's map. The spend is recorded on the note with its fetched spending
    /// transaction, and the wallet is fully scanned, so the cleanup drops the spend from the map.
    #[tokio::test]
    async fn mapped_spend_is_recorded_when_the_funding_block_is_scanned() {
        let spending_transaction = spending_transaction();
        let spending_txid = spending_transaction.txid();
        let mut wallet = wallet_with_mapped_spend(spending_txid);

        let result = process_block(
            &mut wallet,
            spawn_fetcher(Some(&spending_transaction)),
            FUNDING_HEIGHT,
            ScanPriority::Historic,
            funding_scan_results(),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
        assert_eq!(
            wallet.get_wallet_transactions().unwrap()[&spending_txid].status(),
            ConfirmationStatus::Confirmed(SPEND_HEIGHT)
        );
        assert!(wallet.get_wallet_block(FUNDING_HEIGHT).is_ok());
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            scanned_scan_ranges()
        );
        assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
    }

    /// The shielded counterpart of `transparent_spend_without_change::failed_fetch_keeps_the_mapped_spend`.
    ///
    /// The wallet's nullifier map holds the spend of the note the funding block adds, and the request for the
    /// spending transaction fails. The spend must stay in the wallet's map with the note not yet received, so
    /// the next sync session scans the funding block and locates the spend again. Spend detection used to
    /// remove the spend from the map before the request was made, and the note was then left unspent for good.
    #[tokio::test]
    async fn failed_request_keeps_the_mapped_spend() {
        let spending_txid = spending_transaction().txid();
        let mut wallet = wallet_with_mapped_spend(spending_txid);

        let result = process_block(
            &mut wallet,
            spawn_fetcher(None),
            FUNDING_HEIGHT,
            ScanPriority::Historic,
            funding_scan_results(),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            wallet
                .get_nullifiers()
                .unwrap()
                .sapling
                .get(&NOTE_NULLIFIER)
                .and_then(ScanTarget::txid),
            Some(spending_txid),
            "the spend must stay mapped until it is recorded on the note"
        );
        assert!(wallet.get_wallet_transactions().unwrap().is_empty());
        assert!(wallet.get_wallet_block(FUNDING_HEIGHT).is_err());
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            funding_scan_ranges(ScanPriority::Scanning)
        );
    }
}

/// A scan fails the continuity check when the first block of its scan range does not follow the block below it.
mod hash_discontinuity {
    use crate::engine::WalletBlockOps;
    use std::collections::{BTreeMap, HashMap};

    use tokio::sync::mpsc;
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;
    use zl_netutils::lightwallet_protocol::TreeState;

    use crate::{
        client::FetchRequest,
        config::PerformanceLevel,
        error::{ContinuityError, ScanError, SyncError},
        mocks::{MockWallet, MockWalletBuilder, MockWalletError},
        scan::task::{ScanLoad, TaskId},
        sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
        wallet::{SyncState, TreeBounds, WalletBlock, traits::SyncWallet as _},
    };

    use super::NETWORK;
    const BIRTHDAY: u32 = 1;
    /// The first block of the scan range that fails the continuity check.
    const SCAN_RANGE_START: u32 = 21;
    const SCAN_RANGE_END: u32 = 41;
    const VERIFY_BLOCK_RANGE_SIZE: u32 = zl_common::engine::VERIFY_BLOCK_RANGE_SIZE;

    fn scan_range(start: u32, end: u32, priority: ScanPriority) -> ScanRange {
        ScanRange::from_parts(
            BlockHeight::from_u32(start)..BlockHeight::from_u32(end),
            priority,
        )
    }

    fn block(height: u32) -> (BlockHeight, WalletBlock) {
        (
            BlockHeight::from_u32(height),
            WalletBlock::from_parts(
                BlockHeight::from_u32(height),
                BlockHash([0; 32]),
                BlockHash([0; 32]),
                0,
                Vec::new(),
                TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 0,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            ),
        )
    }

    /// Answers tree state requests with empty note commitment trees.
    fn spawn_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
        const EMPTY_TREE: &str = "000000";
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: EMPTY_TREE.to_string(),
                            orchard_tree: EMPTY_TREE.to_string(),
                            ironwood_tree: EMPTY_TREE.to_string(),
                            ..Default::default()
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    fn wallet(scan_ranges: Vec<ScanRange>, blocks: Vec<u32>) -> MockWallet {
        MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(BIRTHDAY))
            .sync_state(SyncState::new_for_test(scan_ranges))
            .wallet_blocks(blocks.into_iter().map(block).collect::<BTreeMap<_, _>>())
            .create_mock_wallet()
    }

    /// Processes the failed scan of `scan_range`, with the continuity check failing at `height`.
    async fn process_hash_discontinuity(
        wallet: &mut MockWallet,
        scan_range: ScanRange,
        height: u32,
        previous_chain_height: BlockHeight,
    ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
        let task_id = TaskId::first();
        let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);

        process_scan_results(
            &NETWORK,
            wallet,
            spawn_fetcher(),
            &HashMap::new(),
            ScanLoad {
                task_id,
                chain_height: scan_range.block_range().end - 1,
                highest_scanned_height: scan_range.block_range().start - 1,
                scan_range,
            },
            &in_flight_tasks,
            Err(ScanError::ContinuityError(
                ContinuityError::HashDiscontinuity {
                    height: BlockHeight::from_u32(height),
                    prev_hash: BlockHash([1; 32]),
                    previous_block_hash: BlockHash([2; 32]),
                },
            )),
            previous_chain_height,
            PerformanceLevel::High,
            &mut false,
        )
        .await
    }

    /// A scan range selected with a priority other than `Verify` is set back to that priority with its first
    /// blocks set to `Verify`, and the height to detect a re-org from is returned for the scanner to verify them.
    #[tokio::test]
    async fn first_blocks_of_a_scan_range_are_verified_again() {
        let mut wallet = wallet(
            vec![
                scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::Scanning),
            ],
            Vec::new(),
        );

        let processed = process_hash_discontinuity(
            &mut wallet,
            scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::ChainTip),
            SCAN_RANGE_START,
            BlockHeight::from_u32(SCAN_RANGE_END - 1),
        )
        .await
        .unwrap();

        assert_eq!(
            processed.reorg_detection_start_height,
            Some(BlockHeight::from_u32(SCAN_RANGE_START))
        );
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            [
                scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                scan_range(
                    SCAN_RANGE_START,
                    SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE,
                    ScanPriority::Verify
                ),
                scan_range(
                    SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE,
                    SCAN_RANGE_END,
                    ScanPriority::ChainTip
                ),
            ]
        );
    }

    /// A hash discontinuity above the first block of the scan range is between blocks the server served together,
    /// or with a scanned block above the scan range. It ends the sync session.
    #[tokio::test]
    async fn hash_discontinuity_above_the_first_block_is_an_error() {
        let scan_ranges = vec![
            scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
            scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::Scanning),
        ];
        let mut wallet = wallet(scan_ranges.clone(), Vec::new());

        let result = process_hash_discontinuity(
            &mut wallet,
            scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::ChainTip),
            SCAN_RANGE_START + 1,
            BlockHeight::from_u32(SCAN_RANGE_END - 1),
        )
        .await;

        assert!(matches!(
            result,
            Err(SyncError::ScanError(ScanError::ContinuityError(
                ContinuityError::HashDiscontinuity { .. }
            )))
        ));
        assert_eq!(wallet.get_sync_state().unwrap().scan_ranges(), scan_ranges);
    }

    /// A re-org detected by a `Verify` scan range truncates the wallet to below the extended verification range,
    /// which removes the wallet data of every block above it. A scanned range above the verification range is
    /// reopened to be scanned again.
    #[tokio::test]
    async fn reorg_reopens_scanned_ranges_above_the_verification_range() {
        const VERIFY_END: u32 = SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE;
        const TRUNCATE_HEIGHT: u32 = SCAN_RANGE_START - VERIFY_BLOCK_RANGE_SIZE - 1;

        let mut wallet = wallet(
            vec![
                scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                scan_range(SCAN_RANGE_START, VERIFY_END, ScanPriority::Scanning),
                scan_range(VERIFY_END, SCAN_RANGE_END, ScanPriority::Scanned),
            ],
            // the bounds of the range that is still scanned after truncation
            vec![BIRTHDAY, TRUNCATE_HEIGHT],
        );

        let processed = process_hash_discontinuity(
            &mut wallet,
            scan_range(SCAN_RANGE_START, VERIFY_END, ScanPriority::Verify),
            SCAN_RANGE_START,
            BlockHeight::from_u32(SCAN_RANGE_END - 1),
        )
        .await
        .unwrap();

        assert_eq!(processed.reorg_detection_start_height, None);
        assert_eq!(
            wallet.get_sync_state().unwrap().scan_ranges(),
            [
                scan_range(BIRTHDAY, TRUNCATE_HEIGHT + 1, ScanPriority::Scanned),
                scan_range(TRUNCATE_HEIGHT + 1, VERIFY_END, ScanPriority::Verify),
                scan_range(VERIFY_END, SCAN_RANGE_END, ScanPriority::Historic),
            ]
        );
    }

    /// A discontinuity more than the re-org allowance below the chain height the wallet's state was formed
    /// against is on a different chain: the block is final on the wallet's chain, so the wallet is cleared
    /// and the chain error returned, with no truncation attempted. A wallet stopped between the loads of a
    /// task meets this when the next session's server disagrees below where it stopped.
    #[tokio::test]
    async fn a_discontinuity_below_the_allowance_of_the_previous_chain_height_is_the_chain_error() {
        const PREVIOUS_CHAIN_HEIGHT: u32 =
            SCAN_RANGE_START + crate::sync::MAX_REORG_ALLOWANCE + VERIFY_BLOCK_RANGE_SIZE;
        let mut wallet = wallet(
            vec![
                scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                scan_range(
                    SCAN_RANGE_START,
                    PREVIOUS_CHAIN_HEIGHT + 1,
                    ScanPriority::Scanning,
                ),
            ],
            vec![BIRTHDAY, SCAN_RANGE_START - 1],
        );

        let result = process_hash_discontinuity(
            &mut wallet,
            scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::Verify),
            SCAN_RANGE_START,
            BlockHeight::from_u32(PREVIOUS_CHAIN_HEIGHT),
        )
        .await;

        assert!(matches!(
            result,
            Err(SyncError::ServerError(
                crate::error::ServerError::ChainVerificationError
            ))
        ));
        assert!(
            wallet.get_sync_state().unwrap().scan_ranges().is_empty(),
            "the wallet is cleared"
        );
    }
}

mod subtree_root_fetch {
    use std::ops::Range;

    use orchard::tree::MerkleHashOrchard;
    use zcash_protocol::consensus::BlockHeight;

    use crate::mocks::orchard_tree_scanned_past_the_fetched_roots;
    use crate::sync::subtree_root_fetch_index;
    use crate::wallet::ShardTrees;
    use crate::witness;

    fn shard_ranges(count: u32) -> Vec<Range<BlockHeight>> {
        (0..count)
            .map(|index| {
                let start = if index == 0 { 1 } else { index * 100 };
                BlockHeight::from_u32(start)..BlockHeight::from_u32((index + 1) * 100 + 1)
            })
            .collect()
    }

    // Scanning carried the tree into shard 4 while the ranges record three roots. The tree would resume
    // from index 5 and never fetch root 3, so the fetch resumes from the newest recorded root instead.
    #[test]
    fn resumes_from_the_newest_recorded_root_when_scanning_carried_the_tree_ahead() {
        let shard_trees = orchard_tree_scanned_past_the_fetched_roots();
        assert_eq!(witness::subtree_fetch_start_index(&shard_trees.orchard), 5);

        assert_eq!(
            subtree_root_fetch_index(&shard_trees.orchard, &shard_ranges(3)),
            2
        );
    }

    // A rollback dropped the tree's bare roots while the ranges still record them, so the fetch resumes
    // from the tree's newest, still bare, root.
    #[test]
    fn resumes_from_the_tree_when_a_rollback_dropped_its_bare_roots() {
        let mut shard_trees = ShardTrees::new();
        let bare_root = zl_netutils::lightwallet_protocol::SubtreeRoot {
            root_hash: <MerkleHashOrchard as incrementalmerkletree::Hashable>::empty_root(
                incrementalmerkletree::Level::from(witness::SHARD_HEIGHT),
            )
            .to_bytes()
            .to_vec(),
            ..Default::default()
        };
        witness::add_subtree_roots(0, vec![bare_root; 2], &mut shard_trees.orchard).unwrap();

        assert_eq!(
            subtree_root_fetch_index(&shard_trees.orchard, &shard_ranges(3)),
            1
        );
    }

    #[test]
    fn a_fresh_or_cleared_pool_fetches_from_the_first_root() {
        let shard_trees = ShardTrees::new();

        assert_eq!(subtree_root_fetch_index(&shard_trees.orchard, &[]), 0);
    }
}

mod pool_rescan {
    use crate::engine::{ScanTargetOps, SyncStateOps, WalletBlockOps};
    use std::collections::{BTreeMap, HashMap};

    use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
    use orchard::tree::MerkleHashOrchard;
    use tokio::sync::mpsc;
    use zcash_primitives::block::BlockHash;
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::consensus::{self, BlockHeight, Parameters as _};
    use zcash_protocol::local_consensus::LocalNetwork;
    use zcash_protocol::{PoolType, ShieldedPool};
    use zl_common::status::ConfirmationStatus;
    use zl_netutils::lightwallet_protocol::TreeState;

    use crate::client::FetchRequest;
    use crate::error::SyncError;
    use crate::mocks::{MockWalletBuilder, subtree_root};
    use crate::sync::{ScanPriority, ScanRange, state, truncate_to_pool_activation_height};
    use crate::wallet::{
        ScanTarget, ScanTargetOrigin, ShardTrees, SyncState, TreeBounds, WalletBlock,
        WalletTransaction,
        traits::{SyncShardTrees, SyncTransactions, SyncWallet},
    };

    /// NU6.3 activates at block 100, so a rescan can straddle the Ironwood boundary.
    const NETWORK: LocalNetwork = LocalNetwork {
        nu6_3: Some(BlockHeight::from_u32(100)),
        ..super::NETWORK
    };

    fn block(height: u32) -> WalletBlock {
        WalletBlock::from_parts(
            BlockHeight::from_u32(height),
            BlockHash([0; 32]),
            BlockHash([0; 32]),
            0,
            Vec::new(),
            TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
        )
    }

    /// Answers tree state requests with empty note commitment trees, omitting the ironwood tree state below the
    /// ironwood activation height.
    fn spawn_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
        const EMPTY_TREE: &str = "000000";
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        let ironwood_tree = if NETWORK
                            .is_nu_active(consensus::NetworkUpgrade::Nu6_3, block_height)
                        {
                            EMPTY_TREE.to_string()
                        } else {
                            String::new()
                        };
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: EMPTY_TREE.to_string(),
                            orchard_tree: EMPTY_TREE.to_string(),
                            ironwood_tree,
                            ..Default::default()
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    /// A pool rescan clears the pool's shard tree, so the pool's shard ranges are cleared with it and both are
    /// rebuilt from the subtree roots fetched from index 0 in the next sync session, so the ranges keep mirroring
    /// the roots the tree holds. The shard trees and shard ranges of pools activated before the rescanned pool
    /// are kept, as their data below the rescanned pool's activation height is not rescanned.
    #[tokio::test]
    async fn rescan_clears_pool_shard_ranges() {
        let mut sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(1)..BlockHeight::from_u32(301),
            ScanPriority::Scanned,
        )]);
        let orchard_shard_ranges = vec![BlockHeight::from_u32(1)..BlockHeight::from_u32(51)];
        sync_state.set_shard_ranges_for_test(ShieldedPool::Orchard, orchard_shard_ranges.clone());
        sync_state.set_shard_ranges_for_test(
            ShieldedPool::Ironwood,
            vec![
                BlockHeight::from_u32(100)..BlockHeight::from_u32(251),
                BlockHeight::from_u32(250)..BlockHeight::from_u32(281),
            ],
        );
        let mut shard_trees = ShardTrees::new();
        shard_trees
            .orchard
            .append(
                MerkleHashOrchard::empty_leaf(),
                Retention::Checkpoint {
                    id: BlockHeight::from_u32(50),
                    marking: Marking::Marked,
                },
            )
            .unwrap();
        let mut wallet = MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(1))
            .sync_state(sync_state)
            .wallet_blocks(BTreeMap::from([(BlockHeight::from_u32(99), block(99))]))
            .shard_trees(shard_trees)
            .create_mock_wallet();

        let result = truncate_to_pool_activation_height(
            &NETWORK,
            spawn_fetcher(),
            &mut wallet,
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(200),
            10,
            5,
        )
        .await;
        assert!(matches!(
            result,
            Ok(SyncError::PoolHistoryReopened {
                pool: PoolType::Shielded(ShieldedPool::Ironwood),
                ..
            })
        ));

        assert_eq!(
            wallet
                .get_shard_trees_mut()
                .unwrap()
                .orchard
                .max_leaf_position(None)
                .unwrap(),
            Some(Position::from(0)),
            "orchard note commitments below the ironwood activation height must be kept"
        );

        let sync_state = wallet.get_sync_state_mut().unwrap();
        assert!(sync_state.ironwood_shard_ranges().is_empty());
        assert_eq!(sync_state.orchard_shard_ranges(), orchard_shard_ranges);

        // the subtree roots fetched from index 0 in the next sync session rebuild the shard ranges
        state::add_shard_ranges(
            &NETWORK,
            ShieldedPool::Ironwood,
            sync_state,
            0,
            &[subtree_root(150), subtree_root(250)],
        );
        assert_eq!(
            sync_state.ironwood_shard_ranges(),
            vec![
                BlockHeight::from_u32(100)..BlockHeight::from_u32(151),
                BlockHeight::from_u32(150)..BlockHeight::from_u32(251),
            ]
        );
    }

    /// The request for the frontier the cleared pool is rebuilt from fails. The wallet keeps the transactions,
    /// shard ranges and scan ranges of the pool's history, so the next sync session finds the same disagreement
    /// and rescans the pool.
    #[tokio::test]
    async fn failed_frontier_request_leaves_the_wallet_as_it_was() {
        const TRANSACTION_HEIGHT: u32 = 200;
        let txid = TxId::from_bytes([1; 32]);
        let scan_ranges = vec![ScanRange::from_parts(
            BlockHeight::from_u32(1)..BlockHeight::from_u32(301),
            ScanPriority::Scanned,
        )];
        let mut sync_state = SyncState::new_for_test(scan_ranges.clone());
        let ironwood_shard_ranges = vec![BlockHeight::from_u32(100)..BlockHeight::from_u32(251)];
        sync_state.set_shard_ranges_for_test(ShieldedPool::Ironwood, ironwood_shard_ranges.clone());
        let mut wallet = MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(1))
            .sync_state(sync_state)
            .wallet_transactions(HashMap::from([(
                txid,
                WalletTransaction::new_for_test(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(TRANSACTION_HEIGHT)),
                ),
            )]))
            .create_mock_wallet();
        let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
        drop(fetch_request_receiver);

        let result = truncate_to_pool_activation_height(
            &NETWORK,
            fetch_request_sender,
            &mut wallet,
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(TRANSACTION_HEIGHT),
            10,
            5,
        )
        .await;

        assert!(result.is_err());
        assert!(
            wallet
                .get_wallet_transactions()
                .unwrap()
                .contains_key(&txid),
            "the transaction must be kept while its scan range is still recorded as scanned"
        );
        let sync_state = wallet.get_sync_state().unwrap();
        assert_eq!(sync_state.ironwood_shard_ranges(), ironwood_shard_ranges);
        assert_eq!(sync_state.scan_ranges(), scan_ranges);
    }

    /// The wallet's records of the pool are wrong, not the chain's, so the transactions the rescan targets
    /// were served and are targeted as the sync engine's own.
    #[tokio::test]
    async fn rescan_targets_the_wallets_transactions_as_the_sync_engines_own() {
        const TRANSACTION_HEIGHT: u32 = 200;
        let txid = TxId::from_bytes([1; 32]);
        let mut wallet = MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(1))
            .sync_state(SyncState::new_for_test(vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(301),
                ScanPriority::Scanned,
            )]))
            .wallet_blocks(BTreeMap::from([(BlockHeight::from_u32(99), block(99))]))
            .wallet_transactions(HashMap::from([(
                txid,
                WalletTransaction::new_for_test(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(TRANSACTION_HEIGHT)),
                ),
            )]))
            .create_mock_wallet();

        truncate_to_pool_activation_height(
            &NETWORK,
            spawn_fetcher(),
            &mut wallet,
            ShieldedPool::Ironwood,
            BlockHeight::from_u32(TRANSACTION_HEIGHT),
            10,
            5,
        )
        .await
        .unwrap();

        let targets = wallet
            .get_sync_state()
            .unwrap()
            .scan_targets()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [ScanTarget::internal(
                BlockHeight::from_u32(TRANSACTION_HEIGHT),
                txid
            )]
        );
        assert_eq!(targets[0].origin(), ScanTargetOrigin::Internal);
    }
}

/// The geometry of a scan range against a block range, which the engine asks at six sites.
mod scan_range_geometry {
    use zcash_protocol::consensus::BlockHeight;

    use crate::sync::{ScanPriority, ScanRange};

    fn blocks(start: u32, end: u32) -> std::ops::Range<BlockHeight> {
        BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
    }

    #[test]
    fn encloses_a_block_range_within_its_bounds() {
        let scan_range = ScanRange::from_parts(blocks(21, 41), ScanPriority::Scanning);

        assert!(scan_range.encloses(&blocks(21, 41)));
        assert!(scan_range.encloses(&blocks(21, 31)));
        assert!(scan_range.encloses(&blocks(31, 41)));
        assert!(!scan_range.encloses(&blocks(20, 31)));
        assert!(!scan_range.encloses(&blocks(31, 42)));
        assert!(!scan_range.encloses(&blocks(1, 21)));
    }

    #[test]
    fn overlaps_a_block_range_that_shares_a_block() {
        let scan_range = ScanRange::from_parts(blocks(21, 41), ScanPriority::Scanning);

        assert!(scan_range.overlaps(&blocks(31, 51)));
        assert!(scan_range.overlaps(&blocks(1, 22)));
        assert!(scan_range.overlaps(&blocks(1, 51)));
        assert!(!scan_range.overlaps(&blocks(41, 51)));
        assert!(!scan_range.overlaps(&blocks(1, 21)));
    }
}

/// The priority a selected scan range is held at while its task is in flight.
mod in_flight_priority {
    use crate::sync::ScanPriority;

    #[test]
    fn refetching_nullifiers_for_scanned_without_mapping_and_scanning_for_every_other_selection() {
        assert_eq!(
            ScanPriority::ScannedWithoutMapping.in_flight(),
            ScanPriority::RefetchingNullifiers
        );
        for selected in [
            ScanPriority::Historic,
            ScanPriority::OpenAdjacent,
            ScanPriority::FoundNote,
            ScanPriority::ChainTip,
            ScanPriority::Verify,
        ] {
            assert_eq!(selected.in_flight(), ScanPriority::Scanning);
        }
    }
}

/// The empty result of a discarded or re-org-handled load.
mod processed_scan_results {
    use crate::sync::ProcessedScanResults;

    #[test]
    fn default_carries_no_addresses() {
        let processed = ProcessedScanResults::default();

        assert!(processed.new_transparent_inuse_addresses.is_empty());
        assert!(processed.new_transparent_gap_addresses.is_empty());
    }
}

/// A session-start request is abandoned when shutdown is requested while it is pending.
mod unless_shutdown {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Duration;

    use crate::sync::{SHUTDOWN_POLL_INTERVAL, unless_shutdown};
    use crate::wallet::SyncMode;

    #[tokio::test]
    async fn a_request_that_completes_while_running_is_served() {
        let sync_mode = AtomicU8::new(SyncMode::Running as u8);

        let outcome = unless_shutdown(&sync_mode, async { 7 }).await.unwrap();

        assert_eq!(outcome, Some(7));
    }

    #[tokio::test]
    async fn a_pending_request_is_abandoned_when_shutdown_is_requested() {
        let sync_mode = AtomicU8::new(SyncMode::Running as u8);
        let request = std::future::pending::<()>();
        let stop = async {
            tokio::time::sleep(SHUTDOWN_POLL_INTERVAL / 2).await;
            sync_mode.store(SyncMode::Shutdown as u8, Ordering::Release);
        };

        let (outcome, ()) = tokio::time::timeout(
            Duration::from_secs(5),
            futures::future::join(unless_shutdown(&sync_mode, request), stop),
        )
        .await
        .expect("the request is abandoned within the poll interval");

        assert_eq!(outcome.unwrap(), None);
    }

    #[tokio::test]
    async fn a_request_pending_under_pause_is_kept() {
        let sync_mode = AtomicU8::new(SyncMode::Paused as u8);
        let request = async {
            tokio::time::sleep(SHUTDOWN_POLL_INTERVAL * 3).await;
            "served"
        };

        let outcome = unless_shutdown(&sync_mode, request).await.unwrap();

        assert_eq!(outcome, Some("served"));
    }
}
