//! The Sync State Contract: a pure validator over [`SyncState`].
//!
//! [`SyncState`] documents invariants on its fields (notably that
//! `scan_ranges` is "in block height order with no overlaps or gaps")
//! but, until this module, nothing checked them: the sync engine
//! preserves them by construction, while [`SyncState::read`] accepted
//! arbitrary bytes from the wallet file unvalidated. A state violating
//! the invariants does not fail loudly — it silently corrupts every
//! consumer that trusts them, from [`SyncState::fully_scanned_height`]
//! to zingolib's spend horizon.
//!
//! [`SyncState::validate`] is the contract's witness: a pure,
//! side-effect-free function that checks every self-contained invariant
//! — every claim decidable from a `&SyncState` alone — and reports the
//! first violation as a [`SyncStateIntegrityError`], one variant per
//! distinct violation, each carrying the evidence. Violations are
//! reported in deterministic field-then-index order: scan ranges first
//! (well-formedness before contiguity at each index), then shard ranges
//! per pool in sapling, orchard, ironwood order, then tree bounds in
//! the same pool order.
//!
//! [`SyncState::read`] enforces the contract at the wallet-file seam: a
//! file whose ranges violate it fails to load with `InvalidData`
//! instead of silently poisoning consumers. Because `read` never
//! restores `initial_sync_state` — session bookkeeping, reset on every
//! load — the tree-bounds invariant guards live and fabricated states
//! rather than files.
//!
//! Four classes of claim are deliberately outside the contract:
//!
//! - Cross-boundary claims — that coverage starts at the wallet
//!   birthday and ends at the chain tip. Those are the wallet's
//!   invariants about its sync state, not the state's invariants about
//!   itself, and checking them requires context this type does not
//!   carry.
//! - `scan_targets` membership — targets are structurally ordered by
//!   `BTreeSet`, and targets outside scan-range coverage are legal:
//!   `clear_all` in zingolib adds targets to a state whose ranges are
//!   still empty.
//! - The `sync_start_height == 0` sentinel semantics, which the engine
//!   overwrites at every sync start.
//! - Bounds on the previously-scanned counts. A mid-session reorg
//!   truncation legally shrinks the scanned ranges below the counts
//!   recorded at session start, so a bound here would convict legally
//!   produced states; the status math saturates its subtractions
//!   instead.

use zcash_protocol::{ShieldedPool, consensus::BlockHeight};

use super::SyncState;

/// A violation of the Sync State Contract: the first invariant breach
/// found in a [`SyncState`], with the evidence that convicts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncStateIntegrityError {
    /// A scan range covers no blocks. There is no inverted-scan-range
    /// variant: [`crate::sync::ScanRange::from_parts`] asserts
    /// `end >= start` and the fields are private, so an inverted scan
    /// range is unrepresentable outside the `sync` module — the corpus
    /// witnesses that with a should-panic test.
    #[error("scan range {index} is empty: {at}..{at}")]
    EmptyScanRange {
        /// Position of the offending range in `scan_ranges`.
        index: usize,
        /// The shared start and end bound.
        at: BlockHeight,
    },
    /// Consecutive scan ranges leave blocks uncovered.
    #[error("scan range {index} ends at {end} but its successor starts at {next_start}: gap")]
    ScanRangeGap {
        /// Position of the earlier range in `scan_ranges`.
        index: usize,
        /// Where the earlier range ends.
        end: BlockHeight,
        /// Where the successor starts, above the end.
        next_start: BlockHeight,
    },
    /// Consecutive scan ranges cover blocks twice. A reordered vec is
    /// also convicted here: ranges out of height order cannot be
    /// contiguous.
    #[error("scan range {index} ends at {end} but its successor starts at {next_start}: overlap")]
    ScanRangeOverlap {
        /// Position of the earlier range in `scan_ranges`.
        index: usize,
        /// Where the earlier range ends.
        end: BlockHeight,
        /// Where the successor starts, below the end.
        next_start: BlockHeight,
    },
    /// A shard range covers no blocks.
    #[error("{pool:?} shard range {index} is empty: {at}..{at}")]
    EmptyShardRange {
        /// The pool whose shard-range vec offends.
        pool: ShieldedPool,
        /// Position of the offending range in that vec.
        index: usize,
        /// The shared start and end bound.
        at: BlockHeight,
    },
    /// A shard range ends before it starts.
    #[error("{pool:?} shard range {index} is inverted: {start}..{end}")]
    InvertedShardRange {
        /// The pool whose shard-range vec offends.
        pool: ShieldedPool,
        /// Position of the offending range in that vec.
        index: usize,
        /// The range's start bound.
        start: BlockHeight,
        /// The range's end bound, below the start.
        end: BlockHeight,
    },
    /// Consecutive shard ranges overlap or are out of height order.
    /// Gaps between shard ranges are legal; overlap and disorder are
    /// the same conviction because a descending pair always overlaps
    /// the ascending reading of the vec.
    #[error(
        "{pool:?} shard range {index} ends at {end} but its successor starts at {next_start}: \
         overlap or disorder"
    )]
    ShardRangeDisorder {
        /// The pool whose shard-range vec offends.
        pool: ShieldedPool,
        /// Position of the earlier range in that vec.
        index: usize,
        /// Where the earlier range ends.
        end: BlockHeight,
        /// Where the successor starts, below the end.
        next_start: BlockHeight,
    },
    /// A pool's initial tree size exceeds its final tree size. The
    /// status math subtracts these on `u32`, so this violation breaks
    /// status queries in any live or fabricated state carrying it.
    #[error(
        "{pool:?} tree bounds are inverted: initial size {initial_tree_size} exceeds final size \
         {final_tree_size}"
    )]
    InvertedTreeBounds {
        /// The pool whose bounds offend.
        pool: ShieldedPool,
        /// The recorded initial tree size.
        initial_tree_size: u32,
        /// The recorded final tree size, below the initial.
        final_tree_size: u32,
    },
}

impl SyncState {
    /// Checks every self-contained invariant of the Sync State
    /// Contract, returning the first violation in deterministic
    /// field-then-index order. See the [module documentation] for the
    /// contract's scope and its deliberate exclusions.
    ///
    /// [module documentation]: self
    pub fn validate(&self) -> Result<(), SyncStateIntegrityError> {
        for (index, scan_range) in self.scan_ranges.iter().enumerate() {
            let range = scan_range.block_range();
            // `>=` rather than `==`: inversion is unrepresentable (the
            // constructor asserts), but if a future `sync`-module bug
            // ever built one, covering no blocks convicts it here too.
            if range.start >= range.end {
                return Err(SyncStateIntegrityError::EmptyScanRange {
                    index,
                    at: range.start,
                });
            }
            if let Some(next) = self.scan_ranges.get(index + 1) {
                let next_start = next.block_range().start;
                if range.end < next_start {
                    return Err(SyncStateIntegrityError::ScanRangeGap {
                        index,
                        end: range.end,
                        next_start,
                    });
                }
                if range.end > next_start {
                    return Err(SyncStateIntegrityError::ScanRangeOverlap {
                        index,
                        end: range.end,
                        next_start,
                    });
                }
            }
        }

        for (pool, shard_ranges) in [
            (ShieldedPool::Sapling, &self.sapling_shard_ranges),
            (ShieldedPool::Orchard, &self.orchard_shard_ranges),
            (ShieldedPool::Ironwood, &self.ironwood_shard_ranges),
        ] {
            for (index, range) in shard_ranges.iter().enumerate() {
                if range.start == range.end {
                    return Err(SyncStateIntegrityError::EmptyShardRange {
                        pool,
                        index,
                        at: range.start,
                    });
                }
                if range.start > range.end {
                    return Err(SyncStateIntegrityError::InvertedShardRange {
                        pool,
                        index,
                        start: range.start,
                        end: range.end,
                    });
                }
                if let Some(next) = shard_ranges.get(index + 1)
                    && range.end > next.start
                {
                    return Err(SyncStateIntegrityError::ShardRangeDisorder {
                        pool,
                        index,
                        end: range.end,
                        next_start: next.start,
                    });
                }
            }
        }

        let bounds = &self.initial_sync_state.wallet_tree_bounds;
        for (pool, initial_tree_size, final_tree_size) in [
            (
                ShieldedPool::Sapling,
                bounds.sapling_initial_tree_size,
                bounds.sapling_final_tree_size,
            ),
            (
                ShieldedPool::Orchard,
                bounds.orchard_initial_tree_size,
                bounds.orchard_final_tree_size,
            ),
            (
                ShieldedPool::Ironwood,
                bounds.ironwood_initial_tree_size,
                bounds.ironwood_final_tree_size,
            ),
        ] {
            if initial_tree_size > final_tree_size {
                return Err(SyncStateIntegrityError::InvertedTreeBounds {
                    pool,
                    initial_tree_size,
                    final_tree_size,
                });
            }
        }

        Ok(())
    }
}

/// The attack corpus: one wallet state per test, each violating a
/// distinct invariant of the Sync State Contract, with control states
/// proving honest wallets pass. The closing test stacks every violation
/// into one maximally adversarial state and peels them off in the
/// validator's documented order.
#[cfg(test)]
// Reversed ranges are not a mistake here: constructing them is the
// corpus's purpose.
#[allow(clippy::reversed_empty_ranges)]
mod attack_corpus {
    use std::ops::Range;

    use super::*;
    use crate::sync::{ScanPriority, ScanRange};

    fn height(h: u32) -> BlockHeight {
        BlockHeight::from_u32(h)
    }

    fn scan_range(range: Range<u32>, priority: ScanPriority) -> ScanRange {
        ScanRange::from_parts(height(range.start)..height(range.end), priority)
    }

    /// A believable mid-sync state: contiguous coverage with a scanned
    /// prefix, plausible shard ranges in every pool, consistent tree
    /// bounds, and a previously-scanned count equal to the scanned
    /// blocks on record.
    fn honest_state() -> SyncState {
        let mut state = SyncState::new();
        state.scan_ranges = vec![
            scan_range(1..361, ScanPriority::Scanned),
            scan_range(361..601, ScanPriority::Historic),
        ];
        state.sapling_shard_ranges = vec![height(10)..height(50), height(80)..height(120)];
        state.orchard_shard_ranges = vec![height(100)..height(200)];
        state.ironwood_shard_ranges = vec![height(400)..height(500)];
        state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_initial_tree_size = 100;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_final_tree_size = 500;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_initial_tree_size = 0;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_final_tree_size = 300;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .ironwood_initial_tree_size = 7;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .ironwood_final_tree_size = 7;
        state.initial_sync_state.previously_scanned_blocks = 360;
        state
    }

    #[test]
    fn the_honest_wallets_pass() {
        assert_eq!(SyncState::new().validate(), Ok(()));
        assert_eq!(honest_state().validate(), Ok(()));
    }

    #[test]
    fn attack_empty_scan_range() {
        let mut state = honest_state();
        state
            .scan_ranges
            .push(scan_range(601..601, ScanPriority::Historic));
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::EmptyScanRange {
                index: 2,
                at: height(601),
            })
        );
    }

    /// An inverted scan range cannot be built at all: the constructor
    /// asserts and the fields are private to the `sync` module. This is
    /// the one attack the corpus mounts against the constructor rather
    /// than the validator, witnessing why the error enum has no
    /// inverted-scan-range variant.
    #[test]
    #[should_panic(expected = "is invalid for ScanRange")]
    fn attack_inverted_scan_range_is_unrepresentable() {
        let _ = scan_range(601..401, ScanPriority::Historic);
    }

    #[test]
    fn attack_scan_range_gap() {
        let mut state = honest_state();
        state
            .scan_ranges
            .push(scan_range(650..700, ScanPriority::Historic));
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::ScanRangeGap {
                index: 1,
                end: height(601),
                next_start: height(650),
            })
        );
    }

    #[test]
    fn attack_scan_range_overlap() {
        let mut state = honest_state();
        state
            .scan_ranges
            .push(scan_range(550..700, ScanPriority::Historic));
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::ScanRangeOverlap {
                index: 1,
                end: height(601),
                next_start: height(550),
            })
        );
    }

    /// Height disorder cannot be built without discontinuity: a
    /// descending pair always reads as an overlap. This attack pins
    /// that the contract convicts a reordered wallet through the
    /// overlap variant rather than letting it pass.
    #[test]
    fn attack_reversed_scan_ranges() {
        let mut state = honest_state();
        state.scan_ranges = vec![
            scan_range(361..601, ScanPriority::Historic),
            scan_range(1..361, ScanPriority::Scanned),
        ];
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::ScanRangeOverlap {
                index: 0,
                end: height(601),
                next_start: height(1),
            })
        );
    }

    #[test]
    fn attack_empty_shard_range() {
        let mut state = honest_state();
        state.orchard_shard_ranges = vec![height(100)..height(100)];
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::EmptyShardRange {
                pool: ShieldedPool::Orchard,
                index: 0,
                at: height(100),
            })
        );
    }

    #[test]
    fn attack_inverted_shard_range() {
        let mut state = honest_state();
        state.sapling_shard_ranges = vec![height(200)..height(100)];
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::InvertedShardRange {
                pool: ShieldedPool::Sapling,
                index: 0,
                start: height(200),
                end: height(100),
            })
        );
    }

    /// Both shapes of the same invariant: a descending pair and an
    /// overlapping ascending pair are the same conviction, because
    /// shard ranges promise only ascending non-overlap (gaps between
    /// shards are legal).
    #[test]
    fn attack_shard_range_disorder() {
        let mut descending = honest_state();
        descending.ironwood_shard_ranges = vec![height(300)..height(400), height(100)..height(200)];
        assert_eq!(
            descending.validate(),
            Err(SyncStateIntegrityError::ShardRangeDisorder {
                pool: ShieldedPool::Ironwood,
                index: 0,
                end: height(400),
                next_start: height(100),
            })
        );

        let mut overlapping = honest_state();
        overlapping.ironwood_shard_ranges =
            vec![height(100)..height(200), height(150)..height(300)];
        assert_eq!(
            overlapping.validate(),
            Err(SyncStateIntegrityError::ShardRangeDisorder {
                pool: ShieldedPool::Ironwood,
                index: 0,
                end: height(200),
                next_start: height(150),
            })
        );
    }

    #[test]
    fn attack_inverted_tree_bounds() {
        let mut state = honest_state();
        state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_initial_tree_size = 500;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_final_tree_size = 100;
        assert_eq!(
            state.validate(),
            Err(SyncStateIntegrityError::InvertedTreeBounds {
                pool: ShieldedPool::Orchard,
                initial_tree_size: 500,
                final_tree_size: 100,
            })
        );
    }

    /// A previously-scanned count exceeding the scanned ranges is NOT a
    /// violation: a mid-session reorg truncation produces it legally,
    /// so the contract must accept it (and the status math saturates
    /// instead of subtracting blindly).
    #[test]
    fn truncation_drift_is_not_an_attack() {
        let mut state = honest_state();
        state.initial_sync_state.previously_scanned_blocks = 500;
        assert_eq!(state.validate(), Ok(()));
    }

    /// The maximally adversarial wallet: every representable invariant
    /// violated at once (inversion of a scan range being
    /// unrepresentable, an empty range and a gap stand in for it). The
    /// test then repairs one violation at a time and watches the next
    /// surface, pinning the validator's documented field-then-index
    /// reporting order.
    #[test]
    fn attack_everything_at_once_peels_in_order() {
        let mut state = SyncState::new();
        state.scan_ranges = vec![
            scan_range(100..100, ScanPriority::Historic),
            scan_range(150..200, ScanPriority::Historic),
        ];
        state.sapling_shard_ranges = vec![height(200)..height(100)];
        state.orchard_shard_ranges = vec![height(50)..height(50)];
        state.ironwood_shard_ranges = vec![height(300)..height(400), height(100)..height(200)];
        state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_initial_tree_size = 9;
        state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_final_tree_size = 1;

        assert!(matches!(
            state.validate(),
            Err(SyncStateIntegrityError::EmptyScanRange { index: 0, .. })
        ));

        state.scan_ranges = vec![scan_range(1..101, ScanPriority::Scanned)];
        assert!(matches!(
            state.validate(),
            Err(SyncStateIntegrityError::InvertedShardRange {
                pool: ShieldedPool::Sapling,
                ..
            })
        ));

        state.sapling_shard_ranges.clear();
        assert!(matches!(
            state.validate(),
            Err(SyncStateIntegrityError::EmptyShardRange {
                pool: ShieldedPool::Orchard,
                ..
            })
        ));

        state.orchard_shard_ranges.clear();
        assert!(matches!(
            state.validate(),
            Err(SyncStateIntegrityError::ShardRangeDisorder {
                pool: ShieldedPool::Ironwood,
                ..
            })
        ));

        state.ironwood_shard_ranges.clear();
        assert!(matches!(
            state.validate(),
            Err(SyncStateIntegrityError::InvertedTreeBounds {
                pool: ShieldedPool::Sapling,
                ..
            })
        ));

        state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_final_tree_size = 9;
        assert_eq!(state.validate(), Ok(()));
    }

    /// The file seam enforces the contract: an honest wallet file
    /// round-trips, while one carrying disordered shard ranges fails to
    /// load as `InvalidData` instead of silently poisoning consumers.
    #[test]
    fn attack_poisoned_wallet_file_fails_to_load() {
        let mut honest = honest_state();
        let mut bytes = Vec::new();
        honest
            .write(&mut bytes)
            .expect("an honest state serializes");
        assert!(SyncState::read(bytes.as_slice()).is_ok());

        let mut poisoned = honest_state();
        poisoned.sapling_shard_ranges = vec![height(300)..height(400), height(100)..height(200)];
        let mut bytes = Vec::new();
        poisoned
            .write(&mut bytes)
            .expect("serialization does not validate; only read does");
        let error = SyncState::read(bytes.as_slice()).expect_err("the contract rejects the file");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// A wallet file whose first scan range is inverted is rejected as
    /// `InvalidData` at read, where it previously panicked inside the
    /// `ScanRange::from_parts` assert before a state ever existed.
    #[test]
    fn attack_inverted_scan_range_bytes_fail_to_load() {
        let mut honest = honest_state();
        let mut bytes = Vec::new();
        honest
            .write(&mut bytes)
            .expect("an honest state serializes");
        // Layout: version (1 byte), CompactSize range count (1 byte for
        // small counts), then the first range's start as u32 LE.
        bytes[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
        let error =
            SyncState::read(bytes.as_slice()).expect_err("inversion is rejected, not a panic");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
