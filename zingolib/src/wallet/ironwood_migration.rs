//! Orchard→Ironwood migration, per the two-phase design of the Ironwood
//! migration ZIP (`draft-schell-ironwood-migration`).
//!
//! **Phase A — note conditioning** (Orchard→Orchard self-sends, both ends
//! shielded, no value revealed): reshape the wallet's Orchard notes into notes
//! sized exactly `denomination + DRAIN_FEE`. Fragmented wallets are folded
//! together with a k-ary batched reduction (≤ [`K_INPUTS`] notes merged per
//! transaction, `~log_K(N)` rounds); large notes are split. Conditioning may
//! run before the NU6.3 activation.
//!
//! **Phase B — drain** (Orchard→Ironwood, pool-crossing, amount revealed):
//! one canonical migration transaction per conditioned note — exactly one
//! Ironwood output whose value is a single power-of-ten denomination, **no
//! change output**, canonical ZIP-317 fee. Because every conditioned note is
//! pre-sized, no drain transaction waits on change from an earlier one, and
//! none emits a wallet-fingerprinting output.
//!
//! The denominations are canonical so that migrated amounts collide across the
//! whole migrating population instead of fingerprinting a wallet:
//!
//! * Denominations are {0.001, 0.01, 0.1, 1, 10, 100} ZEC (powers of ten,
//!   capped at 100 ZEC, floored at 0.001 ZEC).
//! * A balance is decomposed by decimal digit expansion, largest first.
//! * Residue below 0.001 ZEC folds into fees; notes worth ≤ [`SWEEP_MIN`] are
//!   stranded (moving them costs more than they are worth).
//!
//! [`plan_migration`] is the pure planning entry point; it is deterministic
//! and never touches the network, so callers (the one-call
//! `migrate_to_ironwood` orchestration, or a mobile client scheduling steps
//! itself) can present the whole plan for consent before anything is sent.

use zcash_protocol::value::Zatoshis;

/// Number of zatoshis in one ZEC.
const COIN: u64 = 100_000_000;

/// The standard denomination set, in zatoshis, ordered largest first so a
/// greedy decomposition walks it directly.
///
/// {100, 10, 1, 0.1, 0.01, 0.001} ZEC.
pub const DENOMINATIONS: [u64; 6] = [
    100 * COIN,  // 100 ZEC
    10 * COIN,   //  10 ZEC
    COIN,        //   1 ZEC
    COIN / 10,   //   0.1 ZEC
    COIN / 100,  //   0.01 ZEC
    COIN / 1000, //   0.001 ZEC
];

/// The smallest standard denomination, 0.001 ZEC. Any leftover value strictly
/// below this cannot be migrated as a standard note.
pub const SMALLEST_DENOMINATION: u64 = COIN / 1000;

/// The result of decomposing a value into standard denominations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denominations {
    /// One entry per Ironwood output to create. Every value is a member of
    /// [`DENOMINATIONS`]. Ordered largest first.
    outputs: Vec<Zatoshis>,
    /// The sub-0.001-ZEC leftover that has no standard denomination. Always
    /// strictly less than [`SMALLEST_DENOMINATION`]. The migration folds this
    /// into the fee instead of creating a non-standard note.
    remainder: Zatoshis,
}

impl Denominations {
    /// The output notes to create, each a standard denomination, largest first.
    pub fn outputs(&self) -> &[Zatoshis] {
        &self.outputs
    }

    /// The sub-0.001-ZEC leftover to fold into the fee.
    pub fn remainder(&self) -> Zatoshis {
        self.remainder
    }

    /// Total value across all standard-denomination outputs.
    pub fn total(&self) -> Zatoshis {
        // Bounded by the input value, itself a valid `Zatoshis`, so in range.
        Zatoshis::const_from_u64(self.outputs.iter().map(|z| u64::from(*z)).sum())
    }
}

/// Decompose `value` into standard powers-of-ten denominations.
///
/// Greedy, largest denomination first; because each denomination is exactly ten
/// times the next, greedy decomposition also minimizes the number of outputs.
/// The returned [`Denominations::outputs`] sum to `value` minus
/// [`Denominations::remainder`], and the remainder is always below 0.001 ZEC.
///
/// Pass the amount that will actually land in Ironwood — the spendable Orchard
/// total minus the fee. The caller then folds the remainder into the fee, so
/// the wallet drains Orchard to zero.
pub fn decompose(value: Zatoshis) -> Denominations {
    let mut remaining = u64::from(value);
    let mut outputs = Vec::new();

    for denomination in DENOMINATIONS {
        let count = remaining / denomination;
        remaining -= count * denomination;
        for _ in 0..count {
            outputs.push(Zatoshis::const_from_u64(denomination));
        }
    }

    Denominations {
        outputs,
        // `remaining` is what is left after removing every 0.001-ZEC unit, so it
        // is strictly below `SMALLEST_DENOMINATION` and trivially in range.
        remainder: Zatoshis::const_from_u64(remaining),
    }
}

/// ZIP-317 marginal fee per logical action, in zatoshis.
/// [`zcash_primitives::transaction::fees::zip317::MARGINAL_FEE`] as a `u64`,
/// mirrored because `Zatoshis` cannot be unwrapped in `const` context. Pinned
/// to the crate constant by test.
const MARGINAL_FEE: u64 = 5_000;

/// ZIP-317 grace actions, from the crate.
const GRACE_ACTIONS: u64 = zcash_primitives::transaction::fees::zip317::GRACE_ACTIONS as u64;

/// Orchard pads every non-empty bundle to at least this many actions
/// (`orchard::builder`'s `MIN_ACTIONS`, which is private). Pinned to
/// `BundleType::num_actions` by test.
const MIN_BUNDLE_ACTIONS: u64 = 2;

/// Maximum notes merged (inputs) or created (outputs) per conditioning
/// transaction, bounding proving time and transaction size.
pub const K_INPUTS: usize = 100;

/// Notes worth at most this are stranded: spending one contributes less than
/// it costs as a marginal input. Set at 2× the marginal fee so we never move
/// a note for a handful of zatoshis.
pub const SWEEP_MIN: u64 = 2 * MARGINAL_FEE;

/// The canonical ZIP-317 fee of one drain transaction. A drain carries TWO
/// bundles — an Orchard bundle (1 spend, padded to 2 actions) and an Ironwood
/// bundle (1 output, padded to 2 actions) — so it pays for 4 logical actions.
/// Every conditioned note is sized `denomination + DRAIN_FEE` so the drain
/// balances exactly.
pub const DRAIN_FEE: u64 = MARGINAL_FEE * 2 * MIN_BUNDLE_ACTIONS;

/// The ZIP-317 conventional fee for an Orchard-only conditioning transaction
/// with `n_in` spends and `n_out` outputs.
///
/// The action count depends on the era: before NU6.3 activation the Orchard
/// bundle permits cross-address transfers, so a spend and an output can share
/// an action (`max(n_in, n_out)`); from NU6.3 the Orchard bundle disables
/// cross-address transfers and every spend and output occupies its own action
/// (`n_in + n_out`). Both are padded to the bundle minimum of 2.
pub const fn conditioning_fee(n_in: usize, n_out: usize, post_activation: bool) -> u64 {
    let requested = if post_activation {
        (n_in + n_out) as u64
    } else if n_in > n_out {
        n_in as u64
    } else {
        n_out as u64
    };
    let actions = if requested > MIN_BUNDLE_ACTIONS {
        requested
    } else {
        MIN_BUNDLE_ACTIONS
    };
    // GRACE_ACTIONS == MIN_BUNDLE_ACTIONS, so the grace floor is already met.
    let logical = if actions > GRACE_ACTIONS {
        actions
    } else {
        GRACE_ACTIONS
    };
    MARGINAL_FEE * logical
}

/// If a note of value `v` is already drain-ready — sized exactly
/// `denomination + DRAIN_FEE` — returns that denomination.
pub fn drain_denomination(value: u64) -> Option<u64> {
    value
        .checked_sub(DRAIN_FEE)
        .filter(|d| DENOMINATIONS.contains(d))
}

/// One planned Orchard→Orchard conditioning transaction. All values are
/// zatoshis. The fee is implied: `sum(inputs) − sum(outputs)`, always at
/// least the ZIP-317 conventional fee for its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditioningTx {
    /// Values of the notes this transaction spends (≤ [`K_INPUTS`]).
    pub inputs: Vec<u64>,
    /// Values of the self-notes this transaction creates (≤ [`K_INPUTS`]).
    pub outputs: Vec<u64>,
}

impl ConditioningTx {
    /// The implied fee: `sum(inputs) − sum(outputs)`.
    pub fn fee(&self) -> u64 {
        self.inputs.iter().sum::<u64>() - self.outputs.iter().sum::<u64>()
    }
}

/// A complete migration plan: conditioning rounds, then one drain transaction
/// per denomination. Pure data — nothing is signed or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Conditioning rounds, executed in order. Transactions within a round
    /// are independent; each round's outputs must confirm before the next
    /// round spends them.
    pub conditioning_rounds: Vec<Vec<ConditioningTx>>,
    /// The denominations the drain will transfer once conditioning completes
    /// (largest first). Includes denominations of notes that are already
    /// drain-ready. Each costs one more [`DRAIN_FEE`] on top.
    pub drain: Vec<u64>,
    /// Total value stranded in dust notes (each ≤ [`SWEEP_MIN`]) plus any
    /// balance too small to form even the smallest denomination.
    pub stranded: u64,
}

impl MigrationPlan {
    /// True when no conditioning is required and the drain can start now.
    pub fn is_conditioned(&self) -> bool {
        self.conditioning_rounds.is_empty()
    }

    /// Total fees across all conditioning transactions.
    pub fn conditioning_fee(&self) -> u64 {
        self.conditioning_rounds
            .iter()
            .flatten()
            .map(ConditioningTx::fee)
            .sum()
    }

    /// Total fees the drain will pay (one [`DRAIN_FEE`] per denomination).
    pub fn drain_fee(&self) -> u64 {
        self.drain.len() as u64 * DRAIN_FEE
    }
}

/// Plans a full migration for a wallet whose spendable Orchard notes have the
/// given values (zatoshis). Deterministic and pure.
///
/// Notes already sized `denomination + DRAIN_FEE` are left untouched (their
/// denominations appear directly in `drain`), so a migration interrupted and
/// replanned never re-conditions finished notes. Everything else worth more
/// than [`SWEEP_MIN`] is conditioned:
///
/// 1. **Reduction**: while more than [`K_INPUTS`] notes remain, merge groups
///    of ≤ [`K_INPUTS`] notes into one note each (smallest first).
/// 2. **Shaping**: spend the remaining notes into notes sized exactly
///    `denomination + DRAIN_FEE`, where the denominations are the
///    [`decompose`]-ition of what is left after all fees. When the balance
///    needs more than [`K_INPUTS`] target notes (whale balances), shaping
///    recursively splits through intermediate notes, ≤ [`K_INPUTS`] outputs
///    per transaction.
///
/// `post_activation` selects the conditioning fee model (see
/// [`conditioning_fee`]): pass whether the conditioning transactions will
/// confirm at or after the NU6.3 activation height.
pub fn plan_migration(note_values: &[u64], post_activation: bool) -> MigrationPlan {
    let mut drain: Vec<u64> = Vec::new();
    let mut pool: Vec<u64> = Vec::new();
    let mut stranded: u64 = 0;

    for &value in note_values {
        if let Some(denomination) = drain_denomination(value) {
            drain.push(denomination);
        } else if value > SWEEP_MIN {
            pool.push(value);
        } else {
            stranded += value;
        }
    }

    let mut conditioning_rounds: Vec<Vec<ConditioningTx>> = Vec::new();

    // Reduction: k-ary fold until at most K_INPUTS notes remain. Merging the
    // smallest notes first absorbs the most numerous fragments earliest.
    while pool.len() > K_INPUTS {
        pool.sort_unstable();
        let mut round = Vec::new();
        let mut next = Vec::new();
        for group in pool.chunks(K_INPUTS) {
            if group.len() < 2 {
                next.push(group[0]);
                continue;
            }
            let merged =
                group.iter().sum::<u64>() - conditioning_fee(group.len(), 1, post_activation);
            round.push(ConditioningTx {
                inputs: group.to_vec(),
                outputs: vec![merged],
            });
            next.push(merged);
        }
        conditioning_rounds.push(round);
        pool = next;
    }

    // Shaping: find the denominations fundable from the pooled value after
    // all remaining fees, by fixed point (overhead shrinks the decomposable
    // value, which can only shrink the denomination count, so this
    // terminates).
    if !pool.is_empty() {
        let total: u64 = pool.iter().sum();
        let mut decomposable = total;
        let denominations = loop {
            let candidate = decompose(Zatoshis::const_from_u64(decomposable));
            let denominations: Vec<u64> =
                candidate.outputs().iter().map(|z| u64::from(*z)).collect();
            if denominations.is_empty() {
                break denominations;
            }
            let overhead = denominations.len() as u64 * DRAIN_FEE
                + shaping_fee(pool.len(), denominations.len(), post_activation);
            let denomination_sum: u64 = denominations.iter().sum();
            if denomination_sum + overhead <= total {
                break denominations;
            }
            let Some(next) = total.checked_sub(overhead) else {
                break Vec::new();
            };
            decomposable = next;
        };

        if denominations.is_empty() {
            // Not even the smallest denomination is fundable: the pooled
            // value is stranded. Undo any pointless reduction rounds.
            stranded += total + conditioning_rounds
                .iter()
                .flatten()
                .map(ConditioningTx::fee)
                .sum::<u64>();
            conditioning_rounds.clear();
        } else {
            // The pool never contains drain-ready notes (those were diverted
            // into `drain` above), so shaping is always required here.
            let targets: Vec<u64> = denominations.iter().map(|d| d + DRAIN_FEE).collect();
            conditioning_rounds.extend(shape(pool, &targets, post_activation));
            drain.extend(denominations);
        }
    }

    drain.sort_unstable_by(|a, b| b.cmp(a));

    MigrationPlan {
        conditioning_rounds,
        drain,
        stranded,
    }
}

/// Total ZIP-317 fee of shaping `n_in` notes into `m` target notes,
/// recursing through intermediates when `m` exceeds [`K_INPUTS`].
fn shaping_fee(n_in: usize, m: usize, post_activation: bool) -> u64 {
    if m <= K_INPUTS {
        conditioning_fee(n_in, m, post_activation)
    } else {
        // One future transaction per chunk of K_INPUTS targets, funded by an
        // intermediate note each; the intermediates are themselves shaped.
        let chunks = m.div_ceil(K_INPUTS);
        let mut total = 0;
        let mut remaining = m;
        for _ in 0..chunks {
            let len = remaining.min(K_INPUTS);
            total += conditioning_fee(1, len, post_activation);
            remaining -= len;
        }
        total + shaping_fee(n_in, chunks, post_activation)
    }
}

/// The two output shapes a migration transaction can have.
enum MigrationOutputs {
    /// Conditioning: Orchard self-notes with these exact values.
    OrchardSelf(Vec<u64>),
    /// Drain: exactly one Ironwood output of this denomination.
    Ironwood(u64),
}

impl super::LightWallet {
    /// The values (zatoshis) of the account's spendable pre-Ironwood (V2)
    /// Orchard notes — the input to [`plan_migration`].
    #[allow(clippy::result_large_err)]
    pub fn migration_note_values(
        &self,
        account: zip32::AccountId,
    ) -> Result<Vec<u64>, super::error::WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};

        let (_, anchor_height) = self
            .get_migration_heights()?
            .ok_or(super::error::WalletError::NoSyncData)?;
        Ok(self
            .spendable_notes::<pepper_sync::wallet::OrchardNote>(anchor_height, &[], account, false)?
            .into_iter()
            .filter(|note| note.note().version() == orchard::note::NoteVersion::V2)
            .map(|note| note.value())
            .collect())
    }

    /// Builds, proves, signs and records one planned conditioning transaction
    /// (Orchard→Orchard self-send). Returns its txid; broadcast is the
    /// caller's step.
    #[allow(clippy::result_large_err)]
    pub fn build_conditioning_transaction(
        &mut self,
        account: zip32::AccountId,
        planned: &ConditioningTx,
    ) -> Result<zcash_primitives::transaction::TxId, super::error::WalletError> {
        self.build_migration_transaction_inner(
            account,
            &planned.inputs,
            MigrationOutputs::OrchardSelf(planned.outputs.clone()),
        )
    }

    /// Builds, proves, signs and records one canonical drain transaction:
    /// spends the conditioned note valued `denomination + DRAIN_FEE`, creates
    /// exactly one Ironwood output of `denomination`, no change. Returns its
    /// txid; broadcast is the caller's step.
    #[allow(clippy::result_large_err)]
    pub fn build_drain_transaction(
        &mut self,
        account: zip32::AccountId,
        denomination: u64,
    ) -> Result<zcash_primitives::transaction::TxId, super::error::WalletError> {
        self.build_migration_transaction_inner(
            account,
            &[denomination + DRAIN_FEE],
            MigrationOutputs::Ironwood(denomination),
        )
    }

    fn get_migration_heights(
        &self,
    ) -> Result<
        Option<(zcash_protocol::consensus::BlockHeight, zcash_protocol::consensus::BlockHeight)>,
        super::error::WalletError,
    > {
        use zcash_client_backend::data_api::WalletRead as _;
        Ok(self
            .get_target_and_anchor_heights(self.wallet_settings.min_confirmations)
            .expect("infallible")
            .map(|(target, anchor)| (target.into(), anchor)))
    }

    #[allow(clippy::result_large_err)]
    fn build_migration_transaction_inner(
        &mut self,
        account: zip32::AccountId,
        input_values: &[u64],
        outputs: MigrationOutputs,
    ) -> Result<zcash_primitives::transaction::TxId, super::error::WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};
        use zcash_primitives::transaction::builder::{BuildConfig, Builder};
        use zcash_protocol::memo::MemoBytes;

        use super::error::WalletError;

        let (target_height, anchor_height) =
            self.get_migration_heights()?.ok_or(WalletError::NoSyncData)?;

        // Pick one spendable V2 note per planned input value (distinct notes
        // for repeated values), copying out what the builder needs so the
        // wallet borrow ends before witnessing.
        let (notes, positions): (Vec<orchard::Note>, Vec<incrementalmerkletree::Position>) = {
            let mut candidates: Vec<&pepper_sync::wallet::OrchardNote> = self
                .spendable_notes::<pepper_sync::wallet::OrchardNote>(
                    anchor_height,
                    &[],
                    account,
                    false,
                )?
                .into_iter()
                .filter(|note| note.note().version() == orchard::note::NoteVersion::V2)
                .collect();

            let mut notes = Vec::with_capacity(input_values.len());
            let mut positions = Vec::with_capacity(input_values.len());
            for &value in input_values {
                let index = candidates
                    .iter()
                    .position(|note| note.value() == value)
                    .ok_or(WalletError::MigrationNoteNotFound(value))?;
                let selected = candidates.swap_remove(index);
                notes.push(*selected.note());
                positions.push(
                    selected
                        .position()
                        .expect("spendable notes have positions"),
                );
            }
            (notes, positions)
        };

        // Anchor and witnesses from the wallet's Orchard shard tree.
        let orchard_anchor: orchard::Anchor = self
            .shard_trees
            .orchard
            .root_at_checkpoint_id(&anchor_height)?
            .ok_or(WalletError::CheckpointNotFound {
                shielded_protocol: zcash_protocol::ShieldedProtocol::Orchard,
                height: anchor_height,
            })?
            .into();
        let merkle_paths = positions
            .into_iter()
            .map(|position| {
                self.shard_trees
                    .orchard
                    .witness_at_checkpoint_id_caching(position, &anchor_height)?
                    .ok_or(WalletError::CheckpointNotFound {
                        shielded_protocol: zcash_protocol::ShieldedProtocol::Orchard,
                        height: anchor_height,
                    })
                    .map(orchard::tree::MerklePath::from)
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Keys: everything is wallet-internal.
        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&account)
            .ok_or(super::error::KeyError::NoAccountKeys)?
            .try_into()?;
        let orchard_fvk = orchard::keys::FullViewingKey::from(usk.orchard());
        let recipient = orchard_fvk.address_at(0u32, zip32::Scope::Internal);
        let internal_ovk = orchard_fvk.to_ovk(zip32::Scope::Internal);

        // The plan fixes the exact fee (inputs − outputs), which is always at
        // least the ZIP-317 conventional fee and may exceed it where residue
        // was folded in; a non-standard fixed fee rule pins the builder to it.
        let input_sum: u64 = input_values.iter().sum();
        let output_sum: u64 = match &outputs {
            MigrationOutputs::OrchardSelf(values) => values.iter().sum(),
            MigrationOutputs::Ironwood(denomination) => *denomination,
        };
        let fee_rule = zcash_primitives::transaction::fees::fixed::FeeRule::non_standard(
            Zatoshis::from_u64(input_sum - output_sum)?,
        );

        let build_config = BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: Some(orchard_anchor),
            ironwood_anchor: match &outputs {
                // No Ironwood spends exist yet; the empty tree anchors the
                // (output-only) Ironwood bundle.
                MigrationOutputs::Ironwood(_) => Some(orchard::Anchor::empty_tree()),
                MigrationOutputs::OrchardSelf(_) => None,
            },
        };
        let mut builder = Builder::new(self.chain_type, target_height, build_config);
        for (note, merkle_path) in notes.into_iter().zip(merkle_paths) {
            builder
                .add_orchard_spend::<std::convert::Infallible>(
                    orchard_fvk.clone(),
                    note,
                    merkle_path,
                )
                .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
        }
        match &outputs {
            MigrationOutputs::OrchardSelf(values) => {
                for &value in values {
                    builder
                        .add_orchard_output::<std::convert::Infallible>(
                            Some(internal_ovk.clone()),
                            recipient,
                            Zatoshis::from_u64(value)?,
                            MemoBytes::empty(),
                        )
                        .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
                }
            }
            MigrationOutputs::Ironwood(denomination) => {
                builder
                    .add_ironwood_change_output::<std::convert::Infallible>(
                        orchard_fvk.clone(),
                        Some(internal_ovk.clone()),
                        recipient,
                        Zatoshis::from_u64(*denomination)?,
                        MemoBytes::empty(),
                    )
                    .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
            }
        }

        let (sapling_output, sapling_spend) = super::utils::read_sapling_params()
            .map_err(|e| WalletError::MigrationBuild(format!("sapling params: {e}")))?;
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

        // Record the calculated transaction in the wallet (marks the spent
        // notes, stores the tx for `transmit_transactions`), mirroring
        // `store_transactions_to_be_sent`.
        let txid = build_result.transaction().txid();
        let mut transaction_bytes = vec![];
        build_result
            .transaction()
            .write(&mut transaction_bytes)
            .map_err(WalletError::TransactionWrite)?;
        let transaction = zcash_primitives::transaction::Transaction::read(
            transaction_bytes.as_slice(),
            zcash_protocol::consensus::BranchId::for_height(&self.chain_type, target_height),
        )
        .map_err(WalletError::TransactionRead)?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_secs() as u32;
        let chain_type = self.chain_type;
        let ufvks = pepper_sync::wallet::traits::SyncWallet::get_unified_full_viewing_keys(self)?;
        match pepper_sync::scan_pending_transaction(
            &chain_type,
            &ufvks,
            self,
            transaction,
            zingo_status::confirmation_status::ConfirmationStatus::Calculated(target_height),
            timestamp,
        ) {
            Ok(()) => (),
            Err(pepper_sync::error::SyncError::ScanError(e)) => return Err(e.into()),
            Err(pepper_sync::error::SyncError::WalletError(e)) => return Err(e),
            Err(_) => {
                panic!("`scan_pending_transaction` should only return scan or wallet errors")
            }
        }
        self.save_required = true;

        Ok(txid)
    }
}

/// Materializes the shaping rounds that turn `inputs` into exactly `targets`.
/// Mirrors [`shaping_fee`]. The first transaction's fee absorbs any slack
/// between the input total and the exact target-side requirement.
fn shape(inputs: Vec<u64>, targets: &[u64], post_activation: bool) -> Vec<Vec<ConditioningTx>> {
    if targets.len() <= K_INPUTS {
        return vec![vec![ConditioningTx {
            inputs,
            outputs: targets.to_vec(),
        }]];
    }
    let chunks: Vec<&[u64]> = targets.chunks(K_INPUTS).collect();
    let intermediates: Vec<u64> = chunks
        .iter()
        .map(|chunk| chunk.iter().sum::<u64>() + conditioning_fee(1, chunk.len(), post_activation))
        .collect();
    let mut rounds = shape(inputs, &intermediates, post_activation);
    rounds.push(
        intermediates
            .iter()
            .zip(&chunks)
            .map(|(&intermediate, chunk)| ConditioningTx {
                inputs: vec![intermediate],
                outputs: chunk.to_vec(),
            })
            .collect(),
    );
    rounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use zcash_protocol::value::MAX_MONEY;

    fn values(outputs: &[Zatoshis]) -> Vec<u64> {
        outputs.iter().map(|z| u64::from(*z)).collect()
    }

    #[test]
    fn zero_decomposes_to_nothing() {
        let d = decompose(Zatoshis::ZERO);
        assert!(d.outputs().is_empty());
        assert_eq!(d.remainder(), Zatoshis::ZERO);
        assert_eq!(d.total(), Zatoshis::ZERO);
    }

    #[test]
    fn worked_example() {
        // 1.23456789 ZEC: 1 + 2×0.1 + 3×0.01 + 4×0.001, remainder 56_789 zat.
        let d = decompose(Zatoshis::const_from_u64(123_456_789));
        assert_eq!(
            values(d.outputs()),
            vec![
                100_000_000, // 1 ZEC
                10_000_000,
                10_000_000, // 2 × 0.1
                1_000_000,
                1_000_000,
                1_000_000, // 3 × 0.01
                100_000,
                100_000,
                100_000,
                100_000, // 4 × 0.001
            ]
        );
        assert_eq!(u64::from(d.remainder()), 56_789);
    }

    #[test]
    fn value_above_largest_denomination_uses_many_top_notes() {
        // 250 ZEC → 2×100 + 5×10, no remainder.
        let d = decompose(Zatoshis::const_from_u64(250 * COIN));
        assert_eq!(
            values(d.outputs()),
            vec![
                100 * COIN,
                100 * COIN,
                10 * COIN,
                10 * COIN,
                10 * COIN,
                10 * COIN,
                10 * COIN,
            ]
        );
        assert_eq!(d.remainder(), Zatoshis::ZERO);
    }

    #[test]
    fn sub_denomination_value_is_all_remainder() {
        // 0.0009 ZEC is below the smallest denomination: no outputs.
        let d = decompose(Zatoshis::const_from_u64(90_000));
        assert!(d.outputs().is_empty());
        assert_eq!(u64::from(d.remainder()), 90_000);
    }

    #[test]
    fn fee_constants_match_the_zip317_crate() {
        use zcash_primitives::transaction::fees::zip317;
        assert_eq!(MARGINAL_FEE, zip317::MARGINAL_FEE.into_u64());
        assert_eq!(GRACE_ACTIONS, zip317::GRACE_ACTIONS as u64);
    }

    /// Orchard bundle action count for the given spend/output counts, straight
    /// from the orchard crate (which also pins our `MIN_BUNDLE_ACTIONS` mirror
    /// of its private `MIN_ACTIONS`). The flags come from the bundle version's
    /// own defaults — the same choice `zcash_primitives`' builder makes.
    fn crate_bundle_actions(
        n_in: usize,
        n_out: usize,
        version: orchard::bundle::BundleVersion,
    ) -> usize {
        orchard::builder::BundleType::DEFAULT
            .num_actions(version.default_flags(), n_in, n_out)
            .expect("valid action shape")
    }

    /// The crate's conventional fee for a fully-shielded transaction with the
    /// given total logical action count.
    fn crate_fee(actions: usize) -> u64 {
        use zcash_primitives::transaction::fees::{FeeRule as _, zip317};
        zip317::FeeRule::standard()
            .fee_required(
                &zcash_protocol::consensus::MAIN_NETWORK,
                zcash_protocol::consensus::BlockHeight::from_u32(1_000_000),
                std::iter::empty::<zcash_primitives::transaction::fees::transparent::InputSize>(),
                std::iter::empty::<usize>(),
                0,
                0,
                actions,
            )
            .expect("fee computes")
            .into_u64()
    }

    #[test]
    fn conditioning_fee_matches_the_crates() {
        // Pre-activation (orchard_v2 bundle): cross-address transfers
        // permitted → max(in, out). Post-activation (orchard_v3): disabled →
        // in + out. Both padded to 2 per bundle.
        for &(n_in, n_out) in &[(1, 1), (2, 1), (1, 2), (3, 1), (5, 14), (100, 1), (1, 100)] {
            assert_eq!(
                conditioning_fee(n_in, n_out, false),
                crate_fee(crate_bundle_actions(
                    n_in,
                    n_out,
                    orchard::bundle::BundleVersion::orchard_v2()
                )),
                "pre-activation fee mismatch for ({n_in}, {n_out})"
            );
            assert_eq!(
                conditioning_fee(n_in, n_out, true),
                crate_fee(crate_bundle_actions(
                    n_in,
                    n_out,
                    orchard::bundle::BundleVersion::orchard_v3()
                )),
                "post-activation fee mismatch for ({n_in}, {n_out})"
            );
        }
        assert_eq!(conditioning_fee(1, 1, false), 10_000);
        assert_eq!(conditioning_fee(100, 1, false), 500_000);
        assert_eq!(conditioning_fee(100, 1, true), 505_000);
    }

    #[test]
    fn drain_fee_matches_the_crates() {
        // A drain has an Orchard bundle (1 spend, cross-address disabled
        // post-NU6.3) and an Ironwood bundle (1 output, cross-address
        // enabled); the fee covers the sum of both bundles' padded actions.
        let orchard_actions =
            crate_bundle_actions(1, 0, orchard::bundle::BundleVersion::orchard_v3());
        let ironwood_actions =
            crate_bundle_actions(0, 1, orchard::bundle::BundleVersion::ironwood_v3());
        assert_eq!(DRAIN_FEE, crate_fee(orchard_actions + ironwood_actions));
        assert_eq!(DRAIN_FEE, 20_000);
    }

    #[test]
    fn drain_denomination_requires_exact_sizing() {
        assert_eq!(drain_denomination(100_000 + DRAIN_FEE), Some(100_000));
        assert_eq!(drain_denomination(100 * COIN + DRAIN_FEE), Some(100 * COIN));
        assert_eq!(drain_denomination(100_000), None); // fee missing
        assert_eq!(drain_denomination(100_000 + DRAIN_FEE + 1), None);
        assert_eq!(drain_denomination(0), None);
    }

    /// Runs [`assert_plan_executes_in_era`] in both fee eras and returns the
    /// post-activation plan.
    fn assert_plan_executes(notes: &[u64]) -> MigrationPlan {
        assert_plan_executes_in_era(notes, false);
        assert_plan_executes_in_era(notes, true)
    }

    /// Simulates plan execution: every round's inputs must be present in the
    /// wallet's note multiset at that point, and the final multiset must be
    /// exactly the drain-ready notes. Returns the plan for further checks.
    fn assert_plan_executes_in_era(notes: &[u64], post_activation: bool) -> MigrationPlan {
        use std::collections::HashMap;
        let plan = plan_migration(notes, post_activation);

        let mut available: HashMap<u64, i64> = HashMap::new();
        for &note in notes {
            if drain_denomination(note).is_none() && note <= SWEEP_MIN {
                continue; // stranded, never spent
            }
            // With empty conditioning + empty drain, pooled notes are stranded.
            if plan.conditioning_rounds.is_empty()
                && plan.drain.is_empty()
                && drain_denomination(note).is_none()
            {
                continue;
            }
            *available.entry(note).or_insert(0) += 1;
        }

        for round in &plan.conditioning_rounds {
            for tx in round {
                assert!((2..=K_INPUTS).contains(&tx.inputs.len()) || tx.inputs.len() == 1);
                assert!(tx.inputs.len() <= K_INPUTS, "too many inputs");
                assert!(!tx.outputs.is_empty() && tx.outputs.len() <= K_INPUTS);
                let min_fee =
                    conditioning_fee(tx.inputs.len(), tx.outputs.len(), post_activation);
                assert!(tx.fee() >= min_fee, "fee below ZIP-317 conventional");
                for input in &tx.inputs {
                    let count = available.entry(*input).or_insert(0);
                    assert!(*count > 0, "spends a note the wallet does not hold");
                    *count -= 1;
                }
            }
            // Outputs only become spendable after the whole round confirms.
            for tx in round {
                for output in &tx.outputs {
                    assert!(*output > SWEEP_MIN, "conditioning created dust");
                    *available.entry(*output).or_insert(0) += 1;
                }
            }
        }

        // What remains must be exactly the drain set, each note d + DRAIN_FEE.
        let mut expected: HashMap<u64, i64> = HashMap::new();
        for &denomination in &plan.drain {
            *expected.entry(denomination + DRAIN_FEE).or_insert(0) += 1;
        }
        available.retain(|_, count| *count != 0);
        expected.retain(|_, count| *count != 0);
        assert_eq!(available, expected, "post-conditioning notes ≠ drain set");

        plan
    }

    #[test]
    fn already_conditioned_wallet_skips_conditioning() {
        // Notes already sized denomination + fee: no conditioning needed.
        let notes = vec![COIN + DRAIN_FEE, COIN / 10 + DRAIN_FEE, 100_000 + DRAIN_FEE];
        let plan = assert_plan_executes(&notes);
        assert!(plan.is_conditioned());
        assert_eq!(plan.drain, vec![COIN, COIN / 10, 100_000]);
        assert_eq!(plan.stranded, 0);
    }

    #[test]
    fn dust_only_wallet_strands_everything() {
        let notes = vec![5_000, SWEEP_MIN, 1];
        let plan = assert_plan_executes(&notes);
        assert!(plan.conditioning_rounds.is_empty());
        assert!(plan.drain.is_empty());
        assert_eq!(plan.stranded, 5_000 + SWEEP_MIN + 1);
    }

    #[test]
    fn sub_denomination_pool_is_stranded_without_conditioning() {
        // Two sweepable notes that cannot fund even the smallest target.
        let notes = vec![20_000, 30_000];
        let plan = assert_plan_executes(&notes);
        assert!(plan.conditioning_rounds.is_empty());
        assert!(plan.drain.is_empty());
        assert_eq!(plan.stranded, 50_000);
    }

    #[test]
    fn single_note_is_shaped_in_one_transaction() {
        let notes = vec![123_456_789]; // 1.23456789 ZEC
        let plan = assert_plan_executes(&notes);
        assert_eq!(plan.conditioning_rounds.len(), 1);
        assert_eq!(plan.conditioning_rounds[0].len(), 1);
        let tx = &plan.conditioning_rounds[0][0];
        assert_eq!(tx.inputs, vec![123_456_789]);
        // Conservation: everything is either a denomination, a drain fee, or
        // a conditioning fee.
        let denomination_sum: u64 = plan.drain.iter().sum();
        assert_eq!(
            123_456_789,
            denomination_sum + plan.drain_fee() + plan.conditioning_fee() + plan.stranded
        );
    }

    #[test]
    fn fragmented_wallet_reduces_in_rounds() {
        // 500 notes of 0.0015 ZEC: reduction (500 → 5) then shaping.
        let notes = vec![150_000u64; 500];
        let plan = assert_plan_executes(&notes);
        assert!(plan.conditioning_rounds.len() >= 2, "expected reduction + shaping");
        // First round: 5 merges of 100 inputs each.
        assert_eq!(plan.conditioning_rounds[0].len(), 5);
        for tx in &plan.conditioning_rounds[0] {
            assert_eq!(tx.inputs.len(), K_INPUTS);
            assert_eq!(tx.outputs.len(), 1);
        }
        assert!(!plan.drain.is_empty());
    }

    #[test]
    fn whale_balance_splits_through_intermediates() {
        // 5000 ZEC in one note: ~50 denominations of 100 ZEC.
        let notes = vec![5_000 * COIN];
        let plan = assert_plan_executes(&notes);
        assert!(plan.drain.len() >= 49);
        for round in &plan.conditioning_rounds {
            for tx in round {
                assert!(tx.outputs.len() <= K_INPUTS);
            }
        }
    }

    #[test]
    fn very_large_balance_recurses_shaping() {
        // 2,000,000 ZEC: ~20,000 targets of 100 ZEC → recursive shaping
        // through two intermediate levels, ≤ K outputs per tx throughout.
        let notes = vec![2_000_000 * COIN];
        let plan = assert_plan_executes(&notes);
        assert!(plan.drain.len() > K_INPUTS);
        assert!(plan.conditioning_rounds.len() >= 3);
    }

    #[test]
    fn mixed_wallet_leaves_ready_notes_untouched() {
        // One drain-ready note + fragments: the ready note is never spent by
        // conditioning (verified by assert_plan_executes) and its
        // denomination appears in the drain.
        let mut notes = vec![10 * COIN + DRAIN_FEE];
        notes.extend(vec![200_000u64; 50]);
        let plan = assert_plan_executes(&notes);
        assert!(plan.drain.contains(&(10 * COIN)));
    }

    proptest! {
        // Fundamental invariant: the plan executes (round linkage holds, no
        // dust created, ZIP-317 fees respected, ends exactly drain-ready)
        // and value is conserved across the whole migration.
        #[test]
        fn plan_executes_and_conserves_value(
            notes in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
        ) {
            let plan = assert_plan_executes(&notes);
            let total: u64 = notes.iter().sum();
            let denomination_sum: u64 = plan.drain.iter().sum();
            prop_assert_eq!(
                total,
                denomination_sum + plan.drain_fee() + plan.conditioning_fee() + plan.stranded
            );
        }

        // Small-note-heavy wallets (the fragmentation case the reduction
        // rounds exist for).
        #[test]
        fn fragmented_plans_execute(
            notes in proptest::collection::vec(10_001u64..=500_000, 100..400)
        ) {
            let plan = assert_plan_executes(&notes);
            let total: u64 = notes.iter().sum();
            let denomination_sum: u64 = plan.drain.iter().sum();
            prop_assert_eq!(
                total,
                denomination_sum + plan.drain_fee() + plan.conditioning_fee() + plan.stranded
            );
            // log_K bound: 400 notes at K=100 → 1 reduction round + shaping.
            prop_assert!(plan.conditioning_rounds.len() <= 4);
        }
    }

    proptest! {
        // AC: every output is a standard denomination.
        #[test]
        fn outputs_are_standard_denominations(value in 0u64..=MAX_MONEY) {
            let d = decompose(Zatoshis::const_from_u64(value));
            for output in d.outputs() {
                prop_assert!(
                    DENOMINATIONS.contains(&u64::from(*output)),
                    "non-standard output: {output:?}"
                );
            }
        }

        // AC: conservation — outputs + remainder == input.
        #[test]
        fn conserves_value(value in 0u64..=MAX_MONEY) {
            let d = decompose(Zatoshis::const_from_u64(value));
            prop_assert_eq!(
                u64::from(d.total()) + u64::from(d.remainder()),
                value
            );
        }

        // The remainder is always below the smallest denomination, so folding it
        // into the fee costs at most one 0.001-ZEC unit.
        #[test]
        fn remainder_below_smallest_denomination(value in 0u64..=MAX_MONEY) {
            let d = decompose(Zatoshis::const_from_u64(value));
            prop_assert!(u64::from(d.remainder()) < SMALLEST_DENOMINATION);
        }
    }
}
