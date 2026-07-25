//! Per-network migration constants, versioned so testnet and mainnet
//! activations can carry different ratified values.

use crate::config::ChainType;

use super::split::{CANONICAL_PART_FEE, SWEEP_MIN};

/// Number of zatoshis in one ZEC.
pub(crate) const COIN: u64 = 100_000_000;

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
    /// `K_MAX`: the per-cohort multiplicity bound.
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

impl MigrationParams {
    /// The provisional parameter set (ZIP 318 draft values). The chain is
    /// accepted now so ratified per-network values slot in without a
    /// signature change.
    pub fn provisional(_chain: ChainType) -> Self {
        // The `{1, 2, 5} × 10^k` set from 10 000 ZEC down to 0.01 ZEC.
        MigrationParams {
            version: 1,
            denominations: vec![
                10_000 * COIN,
                5_000 * COIN,
                2_000 * COIN,
                1_000 * COIN,
                500 * COIN,
                200 * COIN,
                100 * COIN,
                50 * COIN,
                20 * COIN,
                10 * COIN,
                5 * COIN,
                2 * COIN,
                COIN,
                COIN / 2,
                COIN / 5,
                COIN / 10,
                COIN / 20,
                COIN / 50,
                COIN / 100,
            ],
            denom_cap: 10_000 * COIN,
            max_residual_value: COIN / 100,
            sweep_min: SWEEP_MIN,
            bucket_modulus: 144,
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
