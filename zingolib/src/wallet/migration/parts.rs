//! Part records and their state machine (ZIP 318 Phase 2).
//!
//! A *part* is one canonical migration transaction: a single Orchard spend of
//! a split note and a single Ironwood output of one denomination. Each part
//! is bound to a specific note at split-completion time and walks the state
//! machine below. "Overdue" is deliberately not a state: it is computed by
//! reconciliation from the schedule and the clock, never persisted, so clock
//! skew cannot corrupt stored state.
//!
//! ```text
//! Bound → Assigned → Signed → Broadcast → Confirmed
//!             ↑          │        │
//!             └─ Expired ┴────────┘   (rebuilt: same denomination, fresh bucket)
//! any pre-Confirmed state → Invalidated  (bound note spent outside the migration)
//! ```

use pepper_sync::wallet::OutputId;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use crate::wallet::error::WalletError;

/// Stable identifier of a part within one migration: its index in
/// [`super::MigrationState::parts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartId(pub u32);

/// The Orchard note a part is bound to at split-completion time.
///
/// The `output_id` is the wallet-local handle used to build the transaction.
/// The nullifier is the chain-facing identity used by reconciliation to
/// detect both external spends and parts that mined without being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundNote {
    /// Wallet-local handle: txid and output index of the split note.
    pub output_id: OutputId,
    /// The note's nullifier.
    pub nullifier: [u8; 32],
    /// The note's commitment (cmx), a second identity that survives rescans.
    pub commitment: [u8; 32],
}

/// The immutable anchor data of a part, captured from the wallet's Orchard
/// shard tree as soon as the part's boundary checkpoint is available (the
/// checkpoint retention window is finite, the tree state at a boundary is
/// not). Materialization then needs no tree access at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryWitness {
    /// Orchard root at the boundary: the part's anchor.
    pub anchor: [u8; 32],
    /// The bound note's leaf position.
    pub position: u64,
    /// Merkle authentication path of the bound note at the boundary.
    pub auth_path: Vec<[u8; 32]>,
}

/// How Phase 2 transactions are signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningStrategy {
    /// Build and sign each part at its transmission boundary. The only sound
    /// strategy while ZIP 244 commits the anchor into the signature hash.
    LazyAtBoundary,
    /// Sign every part at consent time and persist the raw transactions.
    /// Becomes sound only if the Ironwood transaction digest excludes the
    /// anchor from the signature hash. Until that consensus question
    /// resolves, `start_ironwood_migration` rejects it.
    PreSigned,
}

/// Where a part is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartState {
    /// Note bound, no bucket assigned yet.
    Bound,
    /// Assigned to a transmission window, with an anchor drawn below it.
    Assigned,
    /// Built and signed (txid and expiry recorded). Transient under
    /// [`SigningStrategy::LazyAtBoundary`], durable under
    /// [`SigningStrategy::PreSigned`].
    Signed,
    /// Submitted to the transmission endpoint at least once.
    Broadcast,
    /// Mined and confirmed at this height. Terminal.
    Confirmed {
        /// The height the part confirmed at.
        height: BlockHeight,
    },
    /// Reached its expiry height unmined. Rebuilt with the same denomination
    /// and a fresh bucket via `PartRecord::reassign`.
    Expired,
    /// The bound note was spent outside the migration. Terminal. The
    /// remaining balance is replanned under fresh consent.
    Invalidated,
}

impl PartState {
    fn name(&self) -> &'static str {
        match self {
            PartState::Bound => "Bound",
            PartState::Assigned => "Assigned",
            PartState::Signed => "Signed",
            PartState::Broadcast => "Broadcast",
            PartState::Confirmed { .. } => "Confirmed",
            PartState::Expired => "Expired",
            PartState::Invalidated => "Invalidated",
        }
    }
}

/// One part of the migration schedule, persisted in the wallet file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartRecord {
    /// Stable identifier (index into the migration's part list).
    pub id: PartId,
    /// The canonical denomination this part transfers, in zatoshis.
    pub denomination: u64,
    /// The split note this part spends. Set at binding time and kept through
    /// every rebuild (the note does not change on expiry, only the anchor).
    pub note: Option<BoundNote>,
    /// The bucket this part transmits in: it is due while the chain tip is
    /// inside the window `[bucket_index · M, (bucket_index + 1) · M)`. The
    /// builder's target height comes from here. Distinct from
    /// `Self::anchor_bucket`, which is where the part *proves*.
    pub bucket_index: Option<u64>,
    /// The bucket whose opening boundary this part anchors its Orchard spend
    /// to, always at least one bucket below [`Self::bucket_index`] (see
    /// [`super::schedule::draw_anchor_bucket`]). Drawn per part at placement
    /// time, so two parts of one batch usually carry different anchors.
    ///
    /// `None` on a part read from a migration section written before anchors
    /// were drawn separately (inner version 3 and below) whose transaction is
    /// not yet signed; the next placement, or
    /// `crate::wallet::LightWallet::refresh_part_witnesses`, draws one.
    pub(crate) anchor_bucket: Option<u64>,
    /// A randomly chosen block within the bucket window at which the part
    /// fires. Randomizing the target within `[boundary, boundary + M)`
    /// prevents the server from seeing all parts cluster at the boundary.
    /// Cleared whenever the bucket changes so a fresh target is chosen.
    pub target_height: Option<BlockHeight>,
    /// Lifecycle state.
    pub state: PartState,
    /// Txid of the built transaction, set when signed.
    pub txid: Option<TxId>,
    /// Expiry height of the built transaction, set when signed.
    pub expiry_height: Option<BlockHeight>,
    /// The raw signed transaction under [`SigningStrategy::PreSigned`].
    /// `None` under the lazy strategy: between signing and transmission the
    /// bytes are recoverable from the wallet's transaction record by txid.
    pub(crate) signed_blob: Option<Vec<u8>>,
    /// The anchor root and witness at `Self::anchor_bucket`'s boundary,
    /// cached while that checkpoint is retained. Cleared on every bucket
    /// transition, since a fresh anchor bucket means a fresh boundary, and
    /// discarded when a pre-anchor-age schedule is read, where it proves the
    /// note under the *window's* boundary instead.
    pub anchor_witness: Option<BoundaryWitness>,
    /// Transmission attempts so far. Incremented (and persisted) before every
    /// submit so a crash between submit and record is detectable.
    pub attempts: u8,
}

impl PartRecord {
    /// A freshly bound part: denomination fixed, note bound, no bucket yet.
    pub fn new(id: PartId, denomination: u64, note: BoundNote) -> Self {
        PartRecord {
            id,
            denomination,
            note: Some(note),
            bucket_index: None,
            anchor_bucket: None,
            target_height: None,
            state: PartState::Bound,
            txid: None,
            expiry_height: None,
            signed_blob: None,
            anchor_witness: None,
            attempts: 0,
        }
    }

    #[allow(clippy::result_large_err)]
    fn transition(&mut self, allowed_from: &[&str], to: PartState) -> Result<(), WalletError> {
        let from = self.state.name();
        if !allowed_from.contains(&from) {
            return Err(WalletError::MigrationInvalidTransition {
                from,
                to: to.name(),
            });
        }
        self.state = to;
        Ok(())
    }

    /// `Bound → Assigned`: the schedule placed this part in a transmission
    /// window.
    ///
    /// Clears `Self::anchor_bucket`, as every bucket transition does: the
    /// placement operations in `super::schedule` are the only writers of an
    /// anchor, and they set it immediately after transitioning. A caller that
    /// assigns directly leaves the part anchorless rather than carrying an
    /// anchor drawn against a different window.
    #[allow(clippy::result_large_err)]
    pub fn assign(&mut self, bucket_index: u64) -> Result<(), WalletError> {
        self.transition(&["Bound"], PartState::Assigned)?;
        self.bucket_index = Some(bucket_index);
        self.anchor_bucket = None;
        Ok(())
    }

    /// `Assigned → Bound`: return the part to the unscheduled pool so a
    /// fresh schedule can re-bucket it (a cadence change before Phase 2
    /// begins). Clears every scheduling artifact. The binding to its note
    /// stands.
    #[allow(clippy::result_large_err)]
    pub(crate) fn unassign(&mut self) -> Result<(), WalletError> {
        self.transition(&["Assigned"], PartState::Bound)?;
        self.bucket_index = None;
        self.anchor_bucket = None;
        self.target_height = None;
        self.anchor_witness = None;
        Ok(())
    }

    /// `Expired → Assigned`: rebuild with the same denomination and note in a
    /// fresh bucket, clearing the stale transaction artifacts (FR15, no fresh
    /// consent needed since the denomination and total are unchanged).
    #[allow(clippy::result_large_err)]
    pub(crate) fn reassign(&mut self, bucket_index: u64) -> Result<(), WalletError> {
        self.transition(&["Expired"], PartState::Assigned)?;
        self.bucket_index = Some(bucket_index);
        self.anchor_bucket = None;
        self.txid = None;
        self.expiry_height = None;
        self.signed_blob = None;
        self.anchor_witness = None;
        Ok(())
    }

    /// `Assigned → Signed`: the canonical transaction is built and signed.
    #[allow(clippy::result_large_err)]
    pub(crate) fn mark_signed(
        &mut self,
        txid: TxId,
        expiry_height: BlockHeight,
        signed_blob: Option<Vec<u8>>,
    ) -> Result<(), WalletError> {
        self.transition(&["Assigned"], PartState::Signed)?;
        self.txid = Some(txid);
        self.expiry_height = Some(expiry_height);
        self.signed_blob = signed_blob;
        Ok(())
    }

    /// Records a submit attempt. Persist the record between this and the
    /// actual submission, so an unrecorded-but-mined part is detectable.
    pub(crate) fn record_attempt(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    /// `Signed → Broadcast`: the transaction was submitted.
    #[allow(clippy::result_large_err)]
    pub(crate) fn mark_broadcast(&mut self) -> Result<(), WalletError> {
        self.transition(&["Signed"], PartState::Broadcast)
    }

    /// `{Assigned, Signed, Broadcast, Expired} → Confirmed`: the bound note's
    /// nullifier was revealed by this part's own transaction. Reachable from
    /// pre-transmission states because a crash between submit and record leaves
    /// the persisted state behind the chain.
    #[allow(clippy::result_large_err)]
    pub(crate) fn mark_confirmed(&mut self, height: BlockHeight) -> Result<(), WalletError> {
        self.transition(
            &["Assigned", "Signed", "Broadcast", "Expired"],
            PartState::Confirmed { height },
        )
    }

    /// `{Assigned, Signed, Broadcast} → Expired`: the signed transaction
    /// reached its expiry height unmined.
    #[allow(clippy::result_large_err)]
    pub(crate) fn mark_expired(&mut self) -> Result<(), WalletError> {
        self.transition(&["Assigned", "Signed", "Broadcast"], PartState::Expired)
    }

    /// `{Bound, Assigned, Signed, Broadcast, Expired} → Invalidated`: the
    /// bound note was spent by a non-migration transaction.
    #[allow(clippy::result_large_err)]
    pub(crate) fn mark_invalidated(&mut self) -> Result<(), WalletError> {
        self.transition(
            &["Bound", "Assigned", "Signed", "Broadcast", "Expired"],
            PartState::Invalidated,
        )
    }

    /// Moves an [`PartState::Assigned`] part to another bucket (schedule
    /// shift after a missed window). Signed parts cannot shift: their anchor
    /// is already fixed to the old boundary, so they expire and reassign
    /// instead.
    #[allow(clippy::result_large_err)]
    pub(crate) fn shift(&mut self, bucket_index: u64) -> Result<(), WalletError> {
        if self.state != PartState::Assigned {
            return Err(WalletError::MigrationInvalidTransition {
                from: self.state.name(),
                to: "Assigned",
            });
        }
        self.bucket_index = Some(bucket_index);
        self.anchor_bucket = None;
        self.target_height = None;
        self.anchor_witness = None;
        Ok(())
    }
}

/// A part's proving work, owning everything the build needs.
///
/// Naming the captured set is the point: a closure hid it in a body, where
/// nothing stated what crossed to the proving thread. Every field is owned,
/// so no wallet reference travels with it and the work runs on any thread.
pub struct ProveOnce {
    /// The account's spending key, for the spend and the output keys.
    usk: zcash_keys::keys::UnifiedSpendingKey,
    /// The historical Orchard root the spend commits to.
    anchor: orchard::Anchor,
    /// The bound note the part spends.
    note: orchard::Note,
    /// That note's path to the anchor.
    merkle_path: orchard::tree::MerklePath,
    /// The chain the transaction is built for.
    chain_type: crate::config::ChainType,
    /// The part's value, which the Ironwood output carries.
    denomination: u64,
    /// The fee this part pays, as a non-standard fixed rule.
    part_fee: u64,
    /// The height whose consensus branch the transaction commits to.
    target_height: BlockHeight,
    /// The height past which the transaction is no longer valid.
    expiry_height: BlockHeight,
    /// The migration's parameters, for the canonical-shape verification.
    params: super::MigrationParams,
}

/// Outcome of `crate::wallet::LightWallet::prepare_part`: either a ready
/// proving closure or the reason the part must be skipped.
pub enum PrepareResult {
    /// All wallet data was extracted. The proving work does the
    /// CPU-intensive part on a background thread.
    Ready {
        /// The proving work, boxed so this variant stays the size of a
        /// pointer rather than of every field the build needs.
        prove: Box<ProveOnce>,
        /// First block of the part's bucket window (the builder's target).
        target_height: BlockHeight,
        /// Expiry height of the built transaction.
        expiry_height: BlockHeight,
    },
    /// The wallet's tree state is not yet ready for this part.
    Skip(SkipReason),
}

/// What materializing a part produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterializeOutcome {
    /// The canonical transaction was built, signed and recorded.
    Materialized {
        /// The transaction's id.
        txid: TxId,
        /// The raw transaction bytes, ready for
        /// [`super::transmission::TransmissionClient::submit`].
        raw_tx: Vec<u8>,
    },
    /// The part cannot be materialized right now. Never triggers a
    /// synchronization. The part falls to reconciliation.
    Skip(SkipReason),
}

/// Why a part was skipped instead of materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The wallet has not scanned as far as the part's boundary, so the
    /// tree state that anchors it is not yet known.
    StaleTreeState {
        /// The height every block at or below which is scanned, if any.
        scanned_to: Option<BlockHeight>,
        /// The boundary the part anchors to.
        boundary: BlockHeight,
    },
    /// Every checkpoint at or below the boundary has been pruned, so the
    /// boundary's tree state is unrecoverable. Reconciliation reassigns the
    /// part to a coming bucket.
    MissedBoundary {
        /// The boundary whose tree state is unavailable.
        boundary: BlockHeight,
    },
    /// The part's anchor boundary predates the NU6.3 activation height. The
    /// window such an anchor belongs to would commit to a pre-NU6.3
    /// consensus branch, in which no Ironwood bundle exists. The schedule's
    /// era floor keeps new placements out of this state and the immediate
    /// path refuses too-young eras up front, so it survives only in a
    /// schedule persisted before the floor existed. Such a part heals
    /// through the overdue classification and the consented catch-up.
    BoundaryBeforeActivation {
        /// The boundary the part anchors to.
        boundary: BlockHeight,
        /// The NU6.3 activation height it fails to reach.
        activation: BlockHeight,
    },
    /// The part carries no anchor bucket: a schedule persisted before
    /// anchors were drawn separately from transmission windows (migration
    /// section inner version 3 and below). Proving cannot invent one,
    /// because the age draw is what keeps the anchor out of the open window.
    /// `crate::wallet::LightWallet::refresh_part_witnesses` draws it at
    /// the next synchronization, which is also when its witness becomes
    /// capturable, so this reason clears itself.
    AnchorNotDrawn,
    /// The bound note is already spent (the user insistently spent it, or a
    /// restart raced an earlier transmission). Reconciliation invalidates the
    /// part and recommends a remainder replan.
    BoundNoteSpent {
        /// The spent note's wallet output id.
        bound: OutputId,
    },
    /// The wallet's record of the bound note no longer matches the part
    /// record (nullifier disagreement). Reconciliation resolves the
    /// divergence.
    BoundNoteMismatch {
        /// The mismatched note's wallet output id.
        bound: OutputId,
    },
}

impl crate::wallet::LightWallet {
    /// The wallet's record of a part's bound note.
    #[allow(clippy::result_large_err)]
    fn bound_orchard_note(
        &self,
        output_id: OutputId,
    ) -> Result<&pepper_sync::wallet::OrchardNote, WalletError> {
        use pepper_sync::wallet::OutputInterface as _;

        self.wallet_transactions
            .get(&output_id.txid())
            .and_then(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
                    .iter()
                    .find(|note| note.output_id() == output_id)
            })
            .ok_or(WalletError::MigrationBoundNoteMissing(output_id))
    }

    /// The confirmation height of one part's bound note, or `None` while it
    /// is unconfirmed or unknown to the wallet.
    ///
    /// This is the lowest boundary *this part* can anchor at: a note has no
    /// Merkle path under an anchor that predates it. It is the per-part half
    /// of [`super::schedule::AnchorFloor`], matching ZIP 318's per-transfer
    /// funding-creation condition. The floor was previously taken as a
    /// maximum across every part, which forced the whole schedule up to the
    /// newest note's boundary; each part now clears only its own.
    #[must_use]
    pub(crate) fn bound_note_confirmed_at(&self, part: &PartRecord) -> Option<BlockHeight> {
        let bound = part.note?;
        self.wallet_transactions
            .get(&bound.output_id.txid())?
            .status()
            .get_confirmed_height()
    }

    /// The checkpoint whose tree state is the tree state at `boundary`: the
    /// greatest one at or below it, or `None` once they are all pruned.
    ///
    /// A bucket boundary is an arbitrary height, and pepper-sync checkpoints
    /// a block only when it carries an Orchard output, so the boundary
    /// usually has no checkpoint of its own. The greatest checkpoint below it
    /// still holds the boundary's tree: had any block in between carried an
    /// Orchard output, that block would itself be a checkpoint. The anchor
    /// this yields is therefore the same value every wallet sending into the
    /// bucket computes, which is what anchoring at the boundary is for.
    ///
    /// Yields nothing until the wallet has scanned past `boundary`: above
    /// the fully scanned height an intervening block's outputs may simply be
    /// missing, and the tree would be short rather than merely older.
    fn boundary_anchor_checkpoint(&self, boundary: BlockHeight) -> Option<BlockHeight> {
        use shardtree::store::ShardStore as _;

        if self
            .sync_state
            .fully_scanned_height()
            .is_none_or(|scanned| scanned < boundary)
        {
            return None;
        }

        let store = self.shard_trees.orchard.store();
        let count = store.checkpoint_count().expect("infallible");
        // Depth counts back from the newest, so the first id at or below the
        // boundary is the greatest one.
        (0..count).find_map(|depth| {
            store
                .get_checkpoint_at_depth(depth)
                .expect("infallible")
                .map(|(id, _)| id)
                .filter(|id| *id <= boundary)
        })
    }

    /// Captures the boundary anchor and witness for one part from the local
    /// shard tree, if a checkpoint holding the boundary's tree survives.
    /// Local-only: never touches the network.
    #[allow(clippy::result_large_err)]
    fn capture_boundary_witness(
        &mut self,
        part: &PartRecord,
        boundary: BlockHeight,
    ) -> Result<Option<BoundaryWitness>, WalletError> {
        use pepper_sync::wallet::NoteInterface as _;

        let bound = part.note.ok_or(WalletError::MigrationStateCorrupt(
            "part has no bound note".to_string(),
        ))?;
        let position = self.bound_orchard_note(bound.output_id)?.position().ok_or(
            WalletError::MigrationStateCorrupt("bound note has no tree position".to_string()),
        )?;

        let Some(checkpoint) = self.boundary_anchor_checkpoint(boundary) else {
            return Ok(None);
        };
        let Some(root) = self
            .shard_trees
            .orchard
            .root_at_checkpoint_id(&checkpoint)?
        else {
            return Ok(None);
        };
        let witness = match self
            .shard_trees
            .orchard
            .witness_at_checkpoint_id_caching(position, &checkpoint)
        {
            Ok(witness) => witness,
            // The checkpoint predates the note, which no anchor there can
            // contain. The same dead end as a pruned checkpoint, and
            // [`super::schedule::AnchorFloor`]'s anchorability floor is what
            // keeps a freshly split batch out of it.
            Err(shardtree::error::ShardTreeError::Query(
                shardtree::error::QueryError::NotContained(_),
            )) => None,
            Err(e) => return Err(e.into()),
        };
        let Some(witness) = witness else {
            return Ok(None);
        };

        Ok(Some(BoundaryWitness {
            anchor: root.to_bytes(),
            position: u64::from(position),
            auth_path: witness
                .path_elems()
                .iter()
                .map(orchard::tree::MerkleHashOrchard::to_bytes)
                .collect(),
        }))
    }

    /// Caches the boundary anchor and witness of every part whose anchor
    /// checkpoint is currently retained. Call after synchronization: the
    /// retention window is finite and a captured witness is good forever.
    ///
    /// Because a part's anchor sits at least one full bucket below its
    /// transmission window, every part gets a whole window's worth of
    /// synchronizations in which to capture its witness before it is due.
    /// That runway is what makes it survivable that pepper-sync checkpoints
    /// wherever Orchard outputs happen to land rather than on the boundary
    /// grid (ADR 0018).
    ///
    /// Also draws the anchor of any part that has none: a schedule persisted
    /// before anchors were drawn separately from windows. Drawing it here
    /// rather than at proving time keeps the placement and the capture in the
    /// same pass, so the part is ready before its window opens.
    ///
    /// A wallet with no witness work — no migration state at all, or no
    /// [`PartState::Assigned`] part awaiting its witness — returns without
    /// consulting the activation schedule, so this ambient post-sync call
    /// never blocks synchronization on a network that never activates NU6.3
    /// (where the start paths refuse loudly, so such work cannot arise).
    /// With work present, a missing NU6.3 activation is a real fault and
    /// errors.
    #[allow(clippy::result_large_err)]
    pub(crate) fn refresh_part_witnesses(&mut self) -> Result<(), WalletError> {
        self.with_migration_state(|wallet, state| {
            let mut pending = state
                .parts
                .iter_mut()
                .filter(|part| part.state == PartState::Assigned && part.anchor_witness.is_none())
                .peekable();
            if pending.peek().is_none() {
                return Ok(());
            }
            let activation = wallet.ironwood_activation()?;
            for part in pending {
                let Some(bucket) = part.bucket_index else {
                    continue;
                };
                if part.anchor_bucket.is_none() {
                    let floor = super::schedule::AnchorFloor::new(
                        activation,
                        wallet.bound_note_confirmed_at(part),
                    );
                    // A legacy part whose window is now too close for any
                    // legal anchor is left alone: reconciliation classifies
                    // it overdue and catch-up re-places it into a window
                    // that has room.
                    if let Some(anchor) = super::schedule::draw_anchor_bucket(
                        bucket,
                        &floor,
                        &mut rand::rngs::OsRng,
                        state.params.bucket_modulus,
                    ) {
                        part.anchor_bucket = Some(anchor);
                        wallet.save_required = true;
                    } else {
                        continue;
                    }
                }
                let anchor_bucket = part
                    .anchor_bucket
                    .expect("just drawn or already present above");
                let boundary =
                    super::schedule::boundary_of(anchor_bucket, state.params.bucket_modulus);
                if let Some(witness) = wallet.capture_boundary_witness(part, boundary)? {
                    part.anchor_witness = Some(witness);
                    wallet.save_required = true;
                }
            }
            Ok(())
        })
        .unwrap_or(Ok(()))
    }

    /// Extracts all wallet data needed to prove one [`PartState::Assigned`]
    /// part and returns it as an owned proving closure. Returns
    /// [`PrepareResult::Skip`] when the part cannot be materialized in this
    /// pass (the tree state is unavailable, the boundary predates the
    /// NU6.3 activation, or the bound note is spent or has diverged from
    /// the part record), so one part's condition never aborts the whole
    /// transmission pass. Skipped parts fall to reconciliation.
    ///
    /// The returned closure does not reference the wallet, so callers can run
    /// multiple closures concurrently on background threads. Mutates
    /// `part.anchor_witness` if the boundary witness needs capturing.
    #[allow(clippy::result_large_err)]
    pub(crate) fn prepare_part(
        &mut self,
        account: zip32::AccountId,
        part: &mut PartRecord,
        params: &super::MigrationParams,
    ) -> Result<PrepareResult, WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};

        if part.state != PartState::Assigned {
            return Err(WalletError::MigrationInvalidTransition {
                from: part.state.name(),
                to: "Signed",
            });
        }
        let window = part
            .bucket_index
            .expect("assigned parts always carry a bucket");
        // The anchor is where the part proves; the window is where it fires
        // and what its target height (and so its consensus branch) comes
        // from. They are different buckets, the anchor always the lower.
        let Some(anchor_bucket) = part.anchor_bucket else {
            return Ok(PrepareResult::Skip(SkipReason::AnchorNotDrawn));
        };
        let boundary = super::schedule::boundary_of(anchor_bucket, params.bucket_modulus);

        let activation = pepper_sync::wallet::PoolActivation::of(
            &self.chain_type,
            zcash_protocol::ShieldedPool::Ironwood,
        )
        .ok_or_else(|| WalletError::MigrationBuild("NU6.3 has no activation height".to_string()))?
        .height();
        if boundary < activation {
            return Ok(PrepareResult::Skip(SkipReason::BoundaryBeforeActivation {
                boundary,
                activation,
            }));
        }

        // The scanned height, not the tree's newest checkpoint: on a chain
        // whose recent blocks carry no Orchard output the tree checkpoints
        // well below a boundary the wallet has long since scanned past, and
        // the boundary's tree state is known all the same.
        let scanned_to = self.sync_state.fully_scanned_height();
        if scanned_to.is_none_or(|scanned| scanned < boundary) {
            return Ok(PrepareResult::Skip(SkipReason::StaleTreeState {
                scanned_to,
                boundary,
            }));
        }

        if part.anchor_witness.is_none() {
            part.anchor_witness = self.capture_boundary_witness(part, boundary)?;
        }
        let Some(boundary_witness) = part.anchor_witness.clone() else {
            return Ok(PrepareResult::Skip(SkipReason::MissedBoundary { boundary }));
        };

        // Revalidate the bound note.
        let bound = part.note.expect("witnessed parts have a bound note");
        let (note, note_value) = {
            let wallet_note = self.bound_orchard_note(bound.output_id)?;
            if wallet_note.note().version() != orchard::note::NoteVersion::V2 {
                return Err(WalletError::MigrationDeviation(
                    "bound note is not a pre-Ironwood (V2) Orchard note".to_string(),
                ));
            }
            if wallet_note
                .nullifier()
                .is_none_or(|nullifier| nullifier.to_bytes() != bound.nullifier)
            {
                return Ok(PrepareResult::Skip(SkipReason::BoundNoteMismatch {
                    bound: bound.output_id,
                }));
            }
            if wallet_note.spending_transaction().is_some() {
                return Ok(PrepareResult::Skip(SkipReason::BoundNoteSpent {
                    bound: bound.output_id,
                }));
            }
            (*wallet_note.note(), wallet_note.value())
        };
        if note_value != part.denomination + params.part_fee {
            return Err(WalletError::MigrationDeviation(format!(
                "bound note value {} is not denomination {} + part fee {}",
                note_value, part.denomination, params.part_fee
            )));
        }

        // Construct the anchor and Merkle path from the cached witness bytes.
        let anchor =
            Option::<orchard::Anchor>::from(orchard::Anchor::from_bytes(boundary_witness.anchor))
                .ok_or_else(|| {
                WalletError::MigrationStateCorrupt("cached boundary anchor is invalid".to_string())
            })?;
        let auth_path = boundary_witness
            .auth_path
            .iter()
            .map(|node| {
                Option::from(orchard::tree::MerkleHashOrchard::from_bytes(node)).ok_or_else(|| {
                    WalletError::MigrationStateCorrupt("cached witness node is invalid".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let merkle_path = orchard::tree::MerklePath::from(
            incrementalmerkletree::MerklePath::from_parts(
                auth_path,
                incrementalmerkletree::Position::from(boundary_witness.position),
            )
            .map_err(|()| {
                WalletError::MigrationStateCorrupt("cached witness is malformed".to_string())
            })?,
        );

        // Extract owned keys from the key store. Wallet access ends here.
        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&account)
            .ok_or(crate::wallet::error::KeyError::NoAccountKeys)?
            .try_into()?;

        let chain_type = self.chain_type;
        let denomination = part.denomination;
        let part_fee = params.part_fee;
        // The target comes from the transmission window, not the anchor. It
        // selects the consensus branch the transaction commits to, so it must
        // sit in the Ironwood era; the anchor is a historical Orchard root
        // and is legal at any retained boundary above the part's own note.
        // Deriving the target from the anchor was what made a pre-activation
        // anchor look like a consensus problem (ADR 0014, ADR 0018).
        let target_height = super::schedule::boundary_of(window, params.bucket_modulus) + 1;
        let expiry_height = super::schedule::canonical_expiry_height(target_height);
        let params_clone = params.clone();

        let prove = Box::new(ProveOnce {
            usk,
            anchor,
            note,
            merkle_path,
            chain_type,
            denomination,
            part_fee,
            target_height,
            expiry_height,
            params: params_clone,
        });

        Ok(PrepareResult::Ready {
            prove,
            target_height,
            expiry_height,
        })
    }

    /// Records a materialized part in the wallet after proving: stores the
    /// transaction, transitions the part to [`PartState::Signed`].
    ///
    /// Called sequentially in Phase C after all parallel proving is complete.
    #[allow(clippy::result_large_err)]
    pub(crate) fn record_part_result(
        &mut self,
        part: &mut PartRecord,
        txid: TxId,
        raw_tx: &[u8],
        target_height: BlockHeight,
        expiry_height: BlockHeight,
        strategy: SigningStrategy,
    ) -> Result<(), WalletError> {
        let recorded = self.record_migration_transaction(raw_tx, target_height)?;
        debug_assert_eq!(
            recorded, txid,
            "txid mismatch between prove and record phases"
        );
        let signed_blob = match strategy {
            SigningStrategy::LazyAtBoundary => None,
            SigningStrategy::PreSigned => Some(raw_tx.to_vec()),
        };
        part.mark_signed(txid, expiry_height, signed_blob)?;
        self.save_required = true;
        Ok(())
    }

    /// Builds, proves, signs and records the canonical transaction for one
    /// [`PartState::Assigned`] part: exactly one Orchard spend of the bound
    /// note, exactly one Ironwood output of the part's denomination, no
    /// other components, the canonical fee, the previous-boundary anchor,
    /// the canonical expiry and `lock_time = 0`. Any deviation is a hard
    /// error, never a fallback.
    ///
    /// Never triggers a synchronization: when the required tree state is
    /// unavailable it returns [`MaterializeOutcome::Skip`] without writing
    /// anything.
    ///
    /// For parallel proving of multiple parts, see `Self::prepare_part` and
    /// `Self::record_part_result`.
    #[allow(clippy::result_large_err)]
    pub fn materialize_part(
        &mut self,
        account: zip32::AccountId,
        part: &mut PartRecord,
        strategy: SigningStrategy,
        params: &super::MigrationParams,
    ) -> Result<MaterializeOutcome, WalletError> {
        match self.prepare_part(account, part, params)? {
            PrepareResult::Skip(reason) => Ok(MaterializeOutcome::Skip(reason)),
            PrepareResult::Ready {
                prove,
                target_height,
                expiry_height,
            } => {
                let (txid, raw_tx) = prove.prove()?;
                self.record_part_result(
                    part,
                    txid,
                    &raw_tx,
                    target_height,
                    expiry_height,
                    strategy,
                )?;
                Ok(MaterializeOutcome::Materialized { txid, raw_tx })
            }
        }
    }
}

impl ProveOnce {
    /// Builds, proves, and signs the part's transaction, yielding its txid
    /// and raw bytes.
    #[allow(clippy::result_large_err)]
    pub(crate) fn prove(self) -> Result<(TxId, Vec<u8>), WalletError> {
        let ProveOnce {
            usk,
            anchor,
            note,
            merkle_path,
            chain_type,
            denomination,
            part_fee,
            target_height,
            expiry_height,
            params: params_clone,
        } = self;
        use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
        use zcash_protocol::memo::MemoBytes;
        use zcash_protocol::value::Zatoshis;

        let orchard_fvk = orchard::keys::FullViewingKey::from(usk.orchard());
        let recipient = orchard_fvk.address_at(0u32, zip32::Scope::Internal);
        let internal_ovk = orchard_fvk.to_ovk(zip32::Scope::Internal);

        let fee_rule = zcash_primitives::transaction::fees::fixed::FeeRule::non_standard(
            Zatoshis::from_u64(part_fee)?,
        );
        let build_config = BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: Some(anchor),
            ironwood_anchor: Some(orchard::Anchor::empty_tree()),
            orchard_padding: BundlePadding::DEFAULT,
            ironwood_padding: BundlePadding::DEFAULT,
        };
        let mut builder =
            Builder::new(chain_type, target_height, build_config).with_expiry_height(expiry_height);
        builder
            .add_orchard_spend::<std::convert::Infallible>(orchard_fvk.clone(), note, merkle_path)
            .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
        builder
            .add_ironwood_output::<std::convert::Infallible>(
                Some(internal_ovk),
                recipient,
                Zatoshis::from_u64(denomination)?,
                MemoBytes::empty(),
            )
            .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;

        let (sapling_output, sapling_spend) = crate::wallet::utils::read_sapling_params();
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let build_result = builder
            .build(
                &zcash_transparent::builder::TransparentSigningSet::new(),
                &[usk.sapling().clone()],
                &[usk.orchard().into()],
                rand::rngs::OsRng,
                &sapling_prover,
                &sapling_prover,
                &fee_rule,
            )
            .map_err(|e| WalletError::MigrationBuild(format!("{e:?}")))?;

        verify_canonical_part(
            build_result.transaction(),
            denomination,
            expiry_height,
            &params_clone,
        )?;

        let txid = build_result.transaction().txid();
        let mut raw_tx = Vec::new();
        build_result
            .transaction()
            .write(&mut raw_tx)
            .map_err(WalletError::TransactionWrite)?;
        Ok((txid, raw_tx))
    }
}

/// Enforces the canonical migration-transaction predicate of ZIP 318. Any
/// deviation is a hard error: a non-canonical part must never leave the
/// wallet, because it would fingerprint it.
#[allow(clippy::result_large_err)]
fn verify_canonical_part(
    transaction: &zcash_primitives::transaction::Transaction,
    denomination: u64,
    expiry_height: BlockHeight,
    params: &super::MigrationParams,
) -> Result<(), WalletError> {
    let deviation = |what: &str| Err(WalletError::MigrationDeviation(what.to_string()));

    if transaction.transparent_bundle().is_some() {
        return deviation("part carries a transparent bundle");
    }
    if transaction.sapling_bundle().is_some() {
        return deviation("part carries a sapling bundle");
    }
    if transaction.lock_time() != 0 {
        return deviation("part has a non-zero lock time");
    }
    if transaction.expiry_height() != expiry_height {
        return deviation("part's expiry is not the canonical expiry");
    }

    let Some(orchard_bundle) = transaction.orchard_bundle() else {
        return deviation("part has no Orchard bundle");
    };
    if orchard_bundle.actions().len() != 2 {
        return deviation("part's Orchard bundle is not padded to two actions");
    }
    let orchard_out = i64::try_from(denomination + params.part_fee).expect("fits i64");
    if i64::from(orchard_bundle.value_balance()) != orchard_out {
        return deviation("part's Orchard value balance is not denomination + fee");
    }

    let Some(ironwood_bundle) = transaction.ironwood_bundle() else {
        return deviation("part has no Ironwood bundle");
    };
    if ironwood_bundle.actions().len() != 2 {
        return deviation("part's Ironwood bundle is not padded to two actions");
    }
    if i64::from(ironwood_bundle.value_balance()) != -i64::try_from(denomination).expect("fits i64")
    {
        return deviation("part's Ironwood value balance is not the denomination");
    }

    Ok(())
}

impl crate::wallet::LightWallet {
    /// Binds every part-ready note to a [`PartRecord`], the switch from
    /// value-based to deterministic note targeting. Called once, when note
    /// splitting completes.
    ///
    /// Every spendable pre-Ironwood (V2) note must either be part-ready
    /// (valued `denomination + part_fee`) or dust below the sweep minimum.
    /// Anything else means splitting has not finished and binding refuses to
    /// proceed. Ready notes become parts largest first, ties broken by
    /// output id, so binding is deterministic for a given note set.
    #[allow(clippy::result_large_err)]
    pub fn bind_parts_to_notes(
        &self,
        state: &mut super::MigrationState,
        account: zip32::AccountId,
    ) -> Result<(), WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};

        use super::split::part_denomination;

        if !state.parts.is_empty() {
            return Err(WalletError::MigrationStateCorrupt(
                "parts are already bound".to_string(),
            ));
        }

        let (_, anchor_height) = self
            .get_migration_heights()?
            .ok_or(WalletError::NoSyncData)?;
        let mut ready: Vec<(u64, BoundNote)> = Vec::new();
        for note in self
            .spendable_notes::<pepper_sync::wallet::OrchardNote>(
                anchor_height,
                &[],
                account,
                false,
            )?
            .into_iter()
            .filter(|note| note.note().version() == orchard::note::NoteVersion::V2)
        {
            match part_denomination(note.value(), &state.params) {
                Some(denomination) => ready.push((
                    denomination,
                    BoundNote {
                        output_id: note.output_id(),
                        nullifier: note
                            .nullifier()
                            .expect("spendable notes have nullifiers")
                            .to_bytes(),
                        commitment: orchard::note::ExtractedNoteCommitment::from(
                            note.note().commitment(),
                        )
                        .to_bytes(),
                    },
                )),
                None if note.value() > state.params.sweep_min => {
                    return Err(WalletError::MigrationStateCorrupt(format!(
                        "cannot bind parts: note of {} zatoshis is neither part-ready nor dust",
                        note.value()
                    )));
                }
                None => (),
            }
        }

        ready
            .sort_by_key(|(denomination, note)| (std::cmp::Reverse(*denomination), note.output_id));
        state.parts = ready
            .into_iter()
            .enumerate()
            .map(|(index, (denomination, note))| {
                PartRecord::new(
                    PartId(u32::try_from(index).expect("part count fits u32")),
                    denomination,
                    note,
                )
            })
            .collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound_note() -> BoundNote {
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([7; 32]), 0),
            nullifier: [1; 32],
            commitment: [2; 32],
        }
    }

    /// Issue #2493, finding 7 (ratified form): placement goes through the
    /// schedule module's operations, and the jittered one draws a fresh
    /// target inside the new window, see
    /// `schedule::tests::place_draws_a_fresh_target_inside_the_new_window`.
    /// The raw `reassign` transition deliberately does not manage the
    /// target. Placement does. Here, the full complement: every state
    /// except `Expired` refuses the transition.
    #[test]
    fn reassign_is_reachable_only_from_expired() {
        for state in ALL_STATES {
            let is_expired = matches!(state, PartState::Expired);
            let mut part = part_in(state);
            if is_expired {
                part.reassign(5).expect("expired parts reassign");
            } else {
                assert!(
                    part.reassign(5).is_err(),
                    "reassign must be illegal from {:?}",
                    part.state,
                );
            }
        }
    }

    fn part_in(state: PartState) -> PartRecord {
        let mut part = PartRecord::new(PartId(0), 100_000, bound_note());
        part.state = state;
        part
    }

    const ALL_STATES: [PartState; 7] = [
        PartState::Bound,
        PartState::Assigned,
        PartState::Signed,
        PartState::Broadcast,
        PartState::Confirmed {
            height: BlockHeight::from_u32(100),
        },
        PartState::Expired,
        PartState::Invalidated,
    ];

    /// Exhaustive legal/illegal edge matrix over every (state, transition)
    /// pair.
    #[test]
    fn transition_matrix() {
        for from in ALL_STATES {
            let legal_assign = matches!(from, PartState::Bound);
            assert_eq!(
                part_in(from).assign(3).is_ok(),
                legal_assign,
                "assign from {from:?}"
            );

            let legal_reassign = matches!(from, PartState::Expired);
            assert_eq!(
                part_in(from).reassign(4).is_ok(),
                legal_reassign,
                "reassign from {from:?}"
            );

            let legal_signed = matches!(from, PartState::Assigned);
            assert_eq!(
                part_in(from)
                    .mark_signed(TxId::from_bytes([9; 32]), BlockHeight::from_u32(500), None)
                    .is_ok(),
                legal_signed,
                "mark_signed from {from:?}"
            );

            let legal_transmission = matches!(from, PartState::Signed);
            assert_eq!(
                part_in(from).mark_broadcast().is_ok(),
                legal_transmission,
                "mark_broadcast from {from:?}"
            );

            let legal_confirmed = matches!(
                from,
                PartState::Assigned | PartState::Signed | PartState::Broadcast | PartState::Expired
            );
            assert_eq!(
                part_in(from)
                    .mark_confirmed(BlockHeight::from_u32(600))
                    .is_ok(),
                legal_confirmed,
                "mark_confirmed from {from:?}"
            );

            let legal_expired = matches!(
                from,
                PartState::Assigned | PartState::Signed | PartState::Broadcast
            );
            assert_eq!(
                part_in(from).mark_expired().is_ok(),
                legal_expired,
                "mark_expired from {from:?}"
            );

            let legal_invalidated =
                !matches!(from, PartState::Confirmed { .. } | PartState::Invalidated);
            assert_eq!(
                part_in(from).mark_invalidated().is_ok(),
                legal_invalidated,
                "mark_invalidated from {from:?}"
            );
        }
    }

    #[test]
    fn reassign_clears_stale_transaction_artifacts() {
        let mut part = part_in(PartState::Assigned);
        part.mark_signed(
            TxId::from_bytes([9; 32]),
            BlockHeight::from_u32(500),
            Some(vec![1, 2, 3]),
        )
        .unwrap();
        part.record_attempt();
        part.mark_broadcast().unwrap();
        part.mark_expired().unwrap();

        part.reassign(7).unwrap();
        assert_eq!(part.state, PartState::Assigned);
        assert_eq!(part.bucket_index, Some(7));
        assert_eq!(part.txid, None);
        assert_eq!(part.expiry_height, None);
        assert_eq!(part.signed_blob, None);
        // The note binding and attempt history survive the rebuild.
        assert!(part.note.is_some());
        assert_eq!(part.attempts, 1);
    }

    #[test]
    fn failed_transition_leaves_the_record_untouched() {
        let mut part = part_in(PartState::Bound);
        let before = part.clone();
        assert!(part.mark_broadcast().is_err());
        assert_eq!(part, before);
    }

    /// A regtest wallet on a chain that never activates NU6.3: the custom-chain
    /// shape whose ambient post-sync refresh must not block synchronization
    /// (branch `fix/refresh_part_witnesses`).
    fn no_nu6_3_wallet() -> crate::wallet::LightWallet {
        let heights = zingo_common_components::protocol::ActivationHeights::builder()
            .set_overwinter(Some(1))
            .set_sapling(Some(1))
            .set_blossom(Some(1))
            .set_heartwood(Some(1))
            .set_canopy(Some(1))
            .set_nu5(Some(1))
            .set_nu6(Some(1))
            .set_nu6_1(Some(1))
            .set_nu6_2(Some(1))
            .set_nu6_3(None)
            .set_nu7(None)
            .build();
        crate::testutils::synthetic_wallet::SyntheticWalletBuilder::new(
            zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED,
        )
        .activation_heights(heights)
        .build()
    }

    fn scheduled_state(parts: Vec<PartRecord>) -> crate::wallet::migration::MigrationState {
        let params = crate::wallet::migration::MigrationParams::provisional(
            crate::config::ChainType::Mainnet,
        );
        crate::wallet::migration::MigrationState {
            consent: crate::wallet::migration::ConsentBinding {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                consented_at: 0,
            },
            params,
            strategy: crate::wallet::migration::SigningStrategy::LazyAtBoundary,
            mode: crate::wallet::migration::MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: crate::wallet::migration::MigrationPhase::PartsScheduled,
            parts,
        }
    }

    /// The inert arm: with no migration state the refresh returns without
    /// consulting the activation schedule, so every sync on a chain without
    /// NU6.3 succeeds. This is the bug the branch fixes — before it, the
    /// lookup ran unconditionally and failed every `await_sync`.
    #[test]
    fn refresh_without_migration_state_is_inert_on_no_nu6_3_chain() {
        let mut wallet = no_nu6_3_wallet();
        wallet
            .refresh_part_witnesses()
            .expect("a wallet with no migration state must sync on a no-NU6.3 chain");
    }

    /// The gate's grain: migration state whose parts carry no witness work
    /// (nothing Assigned-and-unwitnessed) is likewise inert, so terminal or
    /// fully-witnessed migration history never blocks synchronization even
    /// if the activation lookup breaks.
    #[test]
    fn refresh_without_actionable_parts_is_inert_on_no_nu6_3_chain() {
        let mut wallet = no_nu6_3_wallet();
        wallet.migration = Some(scheduled_state(vec![part_in(PartState::Bound)]));
        wallet
            .refresh_part_witnesses()
            .expect("migration state without witness work must not consult the activation");
    }

    /// The loud arm: an Assigned part awaiting its witness on a chain with
    /// no NU6.3 activation can only mean a real fault (the start paths
    /// refuse on such chains), so the refresh errors instead of silently
    /// stalling the migration.
    #[test]
    fn refresh_with_pending_work_errors_on_no_nu6_3_chain() {
        let mut wallet = no_nu6_3_wallet();
        let mut part = PartRecord::new(PartId(0), 100_000, bound_note());
        part.assign(3).expect("bound parts assign");
        wallet.migration = Some(scheduled_state(vec![part]));
        assert!(
            matches!(
                wallet.refresh_part_witnesses(),
                Err(WalletError::MigrationBuild(_))
            ),
            "pending witness work with no NU6.3 activation must error loudly"
        );
    }
}
