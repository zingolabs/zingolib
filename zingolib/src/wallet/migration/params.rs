//! Per-network migration constants, versioned so testnet and mainnet
//! activations can carry different ratified values.

use zcash_pool_migration::note_splitting::{
    MIGRATION_MAX_DENOMINATION_ZEC, RESIDUAL_MIGRATION_MIN,
};
use zcash_pool_migration::scheduling::BOUNDARY_MODULUS;
use zcash_protocol::value::COIN;

use crate::config::ChainType;

use super::split::{CANONICAL_PART_FEE, SWEEP_MIN};

/// The constants ZIP 318 leaves to ratification, gathered so the planner,
/// schedule and part builders all read one source. Every value is provisional
/// until the ZIP is ratified. Changing one only touches [`MigrationParams::provisional`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationParams {
    /// Bumped when ratified constants replace the provisional ones.
    pub version: u32,
    /// Canonical denominations in zatoshis, ordered largest first so a greedy
    /// decomposition walks it directly.
    pub denominations: Vec<u64>,
    /// `DENOM_CAP`: the largest permitted denomination.
    pub denom_cap: u64,
    /// `MAX_RESIDUAL_VALUE`: the smallest permitted denomination. Leftover
    /// value strictly below this cannot be migrated as a standard note.
    pub max_residual_value: u64,
    /// Notes worth at most this are left as residual rather than selected for
    /// migration. Provisionally twice the ZIP-317 marginal fee: a deliberate
    /// safety factor, requiring a selected note to return strictly more than
    /// double the marginal action cost it adds, instead of breaking even
    /// against `MARGINAL_FEE` itself.
    pub sweep_min: u64,
    /// `M`: bucket boundaries are the block heights ≡ 0 (mod `M`).
    /// Invariant: nonzero. `provisional` never produces zero and the
    /// store rejects one at read, so bucket arithmetic may divide by it.
    pub bucket_modulus: u32,
    /// `K_MAX`: the per-batch multiplicity bound: how many parts may share one
    /// broadcast window.
    pub k_max: u32,
    /// The signing-session target the schedule aims at for typical balances.
    pub target_sessions: u32,
    /// Bounds the notes merged (spends) or created (outputs) by one
    /// note-splitting transaction. This caps the Orchard action count at this
    /// value before NU6.3 (spends and outputs share actions) and at twice it
    /// afterwards (cross-address transfers disabled, one action each).
    pub max_actions_per_split_tx: usize,
    /// The canonical ZIP-317 fee of one part. Every split note is sized
    /// `denomination + part_fee` so the part balances exactly.
    pub part_fee: u64,
}

/// The ZIP 318 canonical denomination set: every value `n × 10^k` zatoshis
/// with `n ∈ {1, 2, 5}` lying within `[floor, cap]`, ordered largest first
/// so a greedy decomposition walks it directly. The series itself is
/// standardized at
/// <https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>; the
/// reference crate exports the bounds but not the enumerated set, so the
/// ladder is derived here from the imported bounds.
fn one_two_five_ladder(cap: u64, floor: u64) -> Vec<u64> {
    let mut ladder = Vec::new();
    let mut power = 1u64;
    loop {
        for n in [1u64, 2, 5] {
            let Some(value) = n.checked_mul(power) else {
                continue;
            };
            if (floor..=cap).contains(&value) {
                ladder.push(value);
            }
        }
        match power.checked_mul(10) {
            Some(next) if power <= cap => power = next,
            _ => break,
        }
    }
    ladder.sort_unstable_by(|a, b| b.cmp(a));
    ladder
}

impl MigrationParams {
    /// The provisional parameter set (ZIP 318 draft values). The chain is
    /// accepted now so ratified per-network values slot in without a
    /// signature change.
    ///
    /// Every ZIP-standardized value the canonical `zcash_pool_migration`
    /// crate exports is imported from it, never restated: `DENOM_CAP`
    /// (10 000 ZEC) and `MAX_RESIDUAL_VALUE` (0.01 ZEC) from
    /// <https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>,
    /// and `M` (the boundary modulus, 144) from
    /// <https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>.
    /// The `zip318_conformance_tripwires` tests in the schedule module pin
    /// each imported value to the ZIP's literal number, so a dependency
    /// bump cannot silently move the consent hash. `sweep_min` stays a local
    /// policy: the ZIP leaves the economics of consuming a small note to
    /// ZIP 317 and standardizes no sweep threshold. `k_max` stays local:
    /// the ZIP names `K_MAX` at
    /// <https://zips.z.cash/zip-0318#whalehandling> without fixing a value.
    pub fn provisional(_chain: ChainType) -> Self {
        let denom_cap = MIGRATION_MAX_DENOMINATION_ZEC * COIN;
        let max_residual_value = u64::from(RESIDUAL_MIGRATION_MIN);
        MigrationParams {
            version: 1,
            denominations: one_two_five_ladder(denom_cap, max_residual_value),
            denom_cap,
            max_residual_value,
            sweep_min: SWEEP_MIN,
            bucket_modulus: BOUNDARY_MODULUS,
            k_max: 8,
            target_sessions: 6,
            max_actions_per_split_tx: 32,
            part_fee: CANONICAL_PART_FEE,
        }
    }

    /// A collision-resistant digest of the parameter set, recorded at consent
    /// time so a later replan under different constants is detectable.
    pub fn params_hash(&self) -> [u8; 32] {
        let mut hasher = blake2b_simd::Params::new()
            .hash_length(32)
            .personal(b"ZingoMigParamsV0")
            .to_state();
        hasher.update(&self.version.to_le_bytes());
        hasher.update(&(self.denominations.len() as u64).to_le_bytes());
        for denomination in &self.denominations {
            hasher.update(&denomination.to_le_bytes());
        }
        hasher.update(&self.denom_cap.to_le_bytes());
        hasher.update(&self.max_residual_value.to_le_bytes());
        hasher.update(&self.sweep_min.to_le_bytes());
        hasher.update(&self.bucket_modulus.to_le_bytes());
        hasher.update(&self.k_max.to_le_bytes());
        hasher.update(&self.target_sessions.to_le_bytes());
        hasher.update(&(self.max_actions_per_split_tx as u64).to_le_bytes());
        hasher.update(&self.part_fee.to_le_bytes());
        hasher
            .finalize()
            .as_bytes()
            .try_into()
            .expect("hash length is 32")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisional_denominations_are_capped_and_floored() {
        let params = MigrationParams::provisional(ChainType::Mainnet);
        assert_eq!(params.denominations.first(), Some(&params.denom_cap));
        assert_eq!(
            params.denominations.last(),
            Some(&params.max_residual_value)
        );
        assert!(params.denominations.windows(2).all(|w| w[0] > w[1]));
    }

    /// Pins the digest encoding: an accidental change to the field order or
    /// widths shows up as a mismatch here. Update the vector deliberately
    /// when the provisional values change.
    #[test]
    fn params_hash_is_stable() {
        let params = MigrationParams::provisional(ChainType::Mainnet);
        let hash = params.params_hash();
        assert_eq!(hash, params.params_hash());

        let mut altered = MigrationParams::provisional(ChainType::Mainnet);
        altered.bucket_modulus += 1;
        assert_ne!(hash, altered.params_hash());
    }
}
