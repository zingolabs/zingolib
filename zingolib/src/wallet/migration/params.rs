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
    /// Bumped whenever this parameter set changes — a provisional revision
    /// or the final ratified values — so a stored consent names the exact
    /// set it approved.
    pub version: u32,
    /// Canonical denominations in zatoshis: the values `n × 10^k` ZEC with
    /// `n ∈ {1, 2, 5}`, from the largest pool-crossing denomination
    /// (10000 ZEC) down to `MAX_RESIDUAL_VALUE` (0.01 ZEC), ordered largest
    /// first. Greedy decomposition over this ladder reproduces the ZIP's
    /// decimal digit expansion exactly (see [`super::quantize::decompose`]).
    ///
    /// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
    pub denominations: Vec<u64>,
    /// The largest permitted pool-crossing denomination (10000 ZEC). The
    /// ZIP's named `DENOM_CAP` is this value plus the canonical fee, because
    /// it bounds the funding-note values (denomination plus fee) produced by
    /// note preparation; this field carries the denomination itself.
    ///
    /// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
    pub denom_cap: u64,
    /// `MAX_RESIDUAL_VALUE`: the smallest permitted denomination (0.01 ZEC).
    /// The balance modulo this value is left unmigrated as the residual
    /// rather than crossing as a non-standard note.
    ///
    /// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
    pub dust_floor: u64,
    /// Notes worth at most this are stranded rather than selected for
    /// migration. Provisionally twice the ZIP-317 marginal fee: a deliberate
    /// safety factor, requiring a selected note to return strictly more than
    /// double the marginal action cost it adds, rather than merely break even
    /// against `MARGINAL_FEE` itself.
    pub sweep_min: u64,
    /// `M`: bucket boundaries are the block heights ≡ 0 (mod `M`).
    /// Invariant: nonzero. `provisional` never produces zero and the
    /// store rejects one at read, so bucket arithmetic may divide by it.
    ///
    /// A network-correctness parameter, not a tuning choice: anchor heights
    /// only collide across the migrating population if every wallet shares
    /// one modulus. ZIP 318 provisionally fixes `M = 144` (about three hours
    /// at the 75-second target spacing), equal to the reference
    /// implementation's `MEAN_DELAY`.
    ///
    /// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#anchor-height-bucketing-and-cohorts>
    /// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L95>
    pub bucket_modulus: u32,
    /// Bounds the notes merged (spends) or created (outputs) by one
    /// note-splitting transaction. This caps the Orchard action count at this
    /// value before NU6.3 (spends and outputs share actions) and at twice it
    /// afterwards (cross-address transfers disabled, one action each).
    pub max_actions_per_split_tx: usize,
    /// `EXPIRY_MODULUS`: block-height modulus of the canonical rolling
    /// expiry window (34 560 blocks, about 30 days at the 75-second target
    /// spacing).
    ///
    /// A part's expiry is a pure function of its scheduled broadcast height:
    /// the most recent multiple of this modulus at or below it, plus twice
    /// this modulus (see [`super::schedule::canonical_expiry_height`]).
    /// Every migration transaction scheduled in the same 30-day period —
    /// from any wallet — commits the identical expiry height, so the expiry
    /// carries no per-wallet information. This field replaced the
    /// boundary-relative `expiry_delta` of params version 0 in the same
    /// serialized slot.
    ///
    /// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#canonical-migration-transaction-structure>
    /// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L106-L112>
    pub expiry_modulus: u32,
    /// The canonical ZIP-317 fee of one part. Every split note is sized
    /// `denomination + part_fee` so the part balances exactly.
    pub part_fee: u64,
}

impl MigrationParams {
    /// The provisional parameter set (ZIP 318 draft values). The chain is
    /// accepted now so ratified per-network values slot in without a
    /// signature change.
    pub fn provisional(_chain: ChainType) -> Self {
        MigrationParams {
            // Version 1: `bucket_modulus` matched to the ZIP's network-wide
            // `M = 144`, the boundary-relative `expiry_delta` replaced by
            // the canonical rolling `expiry_modulus` (issue #2519,
            // deviations 3 and 4), and the denomination set widened to the
            // ZIP's full `{1, 2, 5} × 10^k` ladder between 0.01 and
            // 10000 ZEC (previously powers of ten between 0.001 and 100).
            // The former `k_max` per-cohort multiplicity bound is gone
            // (issue #2519, deviation 5): ZIP 318 deliberately places no
            // cap on per-wallet multiplicity, since truncating the outcome
            // of random draws with an arbitrary bound would only distort
            // the distribution.
            // <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#a-note-on-cohort-size-vs-per-wallet-multiplicity>
            // <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L40-L45>
            version: 1,
            denominations: vec![
                10_000 * COIN, // 10000 ZEC, the largest crossing denomination
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
                COIN,       // 1 ZEC
                COIN / 2,   // 0.5 ZEC
                COIN / 5,   // 0.2 ZEC
                COIN / 10,  // 0.1 ZEC
                COIN / 20,  // 0.05 ZEC
                COIN / 50,  // 0.02 ZEC
                COIN / 100, // 0.01 ZEC = MAX_RESIDUAL_VALUE
            ],
            denom_cap: 10_000 * COIN,
            dust_floor: COIN / 100,
            sweep_min: SWEEP_MIN,
            bucket_modulus: 144,
            max_actions_per_split_tx: 32,
            expiry_modulus: 34_560,
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
        hasher.update(&self.dust_floor.to_le_bytes());
        hasher.update(&self.sweep_min.to_le_bytes());
        hasher.update(&self.bucket_modulus.to_le_bytes());
        hasher.update(&(self.max_actions_per_split_tx as u64).to_le_bytes());
        hasher.update(&self.expiry_modulus.to_le_bytes());
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
        assert_eq!(params.denominations.last(), Some(&params.dust_floor));
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
