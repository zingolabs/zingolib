//! Canonical quantization: decomposing a balance into the standard
//! power-of-ten denominations of ZIP 318.

use zcash_protocol::value::Zatoshis;

use super::params::MigrationParams;

/// The result of decomposing a value into canonical denominations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denominations {
    /// One entry per Ironwood output to create. Every value is a member of
    /// [`MigrationParams::denominations`]. Ordered largest first.
    outputs: Vec<Zatoshis>,
    /// The leftover below the dust floor that has no canonical denomination.
    /// Always strictly less than [`MigrationParams::dust_floor`]. The
    /// migration folds this into the fee instead of creating a non-standard
    /// note.
    remainder: Zatoshis,
}

impl Denominations {
    /// The output notes to create, each a canonical denomination, largest first.
    pub fn outputs(&self) -> &[Zatoshis] {
        &self.outputs
    }

    /// The sub-dust-floor leftover to fold into the fee.
    pub fn remainder(&self) -> Zatoshis {
        self.remainder
    }

    /// Total value across all canonical outputs.
    pub fn total(&self) -> Zatoshis {
        // Bounded by the input value, itself a valid `Zatoshis`, so in range.
        Zatoshis::const_from_u64(self.outputs.iter().map(|z| u64::from(*z)).sum())
    }
}

/// Decompose `value` into canonical powers-of-ten denominations.
///
/// Greedy, largest denomination first. Because each denomination is exactly
/// ten times the next, greedy decomposition also minimizes the number of
/// outputs. The returned [`Denominations::outputs`] sum to `value` minus
/// [`Denominations::remainder`], and the remainder is always below the dust
/// floor.
///
/// Pass the amount that will actually land in Ironwood, the spendable Orchard
/// total minus the fee. The caller then folds the remainder into the fee, so
/// the wallet empties the Orchard pool.
pub fn decompose(value: Zatoshis, params: &MigrationParams) -> Denominations {
    let mut remaining = u64::from(value);
    let mut outputs = Vec::new();

    for &denomination in &params.denominations {
        let count = remaining / denomination;
        remaining -= count * denomination;
        for _ in 0..count {
            outputs.push(Zatoshis::const_from_u64(denomination));
        }
    }

    Denominations {
        outputs,
        // `remaining` is what is left after removing every dust-floor unit,
        // so it is strictly below the dust floor and trivially in range.
        remainder: Zatoshis::const_from_u64(remaining),
    }
}

#[cfg(test)]
mod tests {
    use super::super::params::COIN;
    use super::*;
    use crate::config::ChainType;
    use proptest::prelude::*;
    use zcash_protocol::value::MAX_MONEY;

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    fn values(outputs: &[Zatoshis]) -> Vec<u64> {
        outputs.iter().map(|z| u64::from(*z)).collect()
    }

    #[test]
    fn zero_decomposes_to_nothing() {
        let d = decompose(Zatoshis::ZERO, &params());
        assert!(d.outputs().is_empty());
        assert_eq!(d.remainder(), Zatoshis::ZERO);
        assert_eq!(d.total(), Zatoshis::ZERO);
    }

    #[test]
    fn worked_example() {
        // 1.23456789 ZEC: 1 + 2×0.1 + 3×0.01 + 4×0.001, remainder 56_789 zat.
        let d = decompose(Zatoshis::const_from_u64(123_456_789), &params());
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
        let d = decompose(Zatoshis::const_from_u64(250 * COIN), &params());
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
        let d = decompose(Zatoshis::const_from_u64(90_000), &params());
        assert!(d.outputs().is_empty());
        assert_eq!(u64::from(d.remainder()), 90_000);
    }

    proptest! {
        // AC: every output is a canonical denomination.
        #[test]
        fn outputs_are_canonical_denominations(value in 0u64..=MAX_MONEY) {
            let params = params();
            let d = decompose(Zatoshis::const_from_u64(value), &params);
            for output in d.outputs() {
                prop_assert!(
                    params.denominations.contains(&u64::from(*output)),
                    "non-canonical output: {output:?}"
                );
            }
        }

        // AC: conservation, outputs + remainder == input.
        #[test]
        fn conserves_value(value in 0u64..=MAX_MONEY) {
            let d = decompose(Zatoshis::const_from_u64(value), &params());
            prop_assert_eq!(
                u64::from(d.total()) + u64::from(d.remainder()),
                value
            );
        }

        // The remainder is always below the dust floor, so folding it into
        // the fee costs at most one dust-floor unit.
        #[test]
        fn remainder_below_dust_floor(value in 0u64..=MAX_MONEY) {
            let params = params();
            let d = decompose(Zatoshis::const_from_u64(value), &params);
            prop_assert!(u64::from(d.remainder()) < params.dust_floor);
        }
    }
}
