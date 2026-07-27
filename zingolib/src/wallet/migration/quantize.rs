//! Canonical quantization: decomposing a balance into the standard
//! `{1, 2, 5} × 10^k` denominations of ZIP 318.

use zcash_protocol::value::Zatoshis;

use super::params::MigrationParams;

/// The result of decomposing a value into canonical denominations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denominations {
    /// One entry per Ironwood output to create. Every value is a member of
    /// [`MigrationParams::denominations`]. Ordered largest first.
    outputs: Vec<Zatoshis>,
    /// The leftover below the smallest denomination that has no canonical denomination.
    /// Always strictly less than [`MigrationParams::max_residual_value`]. The
    /// migration folds this into the fee instead of creating a non-standard
    /// note.
    remainder: Zatoshis,
}

impl Denominations {
    /// The output notes to create, each a canonical denomination, largest first.
    pub fn outputs(&self) -> &[Zatoshis] {
        &self.outputs
    }

    /// The sub-denomination leftover to fold into the fee.
    pub fn remainder(&self) -> Zatoshis {
        self.remainder
    }

    /// Total value across all canonical outputs.
    pub fn total(&self) -> Zatoshis {
        // Bounded by the input value, itself a valid `Zatoshis`, so in range.
        Zatoshis::const_from_u64(self.outputs.iter().map(|z| u64::from(*z)).sum())
    }
}

/// Decompose `value` into canonical `{1, 2, 5} × 10^k` denominations.
///
/// Greedy, largest denomination first. Over the full `{1, 2, 5}` ladder this
/// is exactly the ZIP's baseline quantization by decimal digit expansion:
/// each digit of the balance expands greedily into `{5, 2, 1}` parts of its
/// place value (a digit of 9 yields `5, 2, 2`), and value above the largest
/// denomination becomes repeated maximal-denomination parts. The returned
/// [`Denominations::outputs`] sum to `value` minus
/// [`Denominations::remainder`], and the remainder is always below
/// `MAX_RESIDUAL_VALUE` (the smallest denomination). The ZIP's optional
/// randomized decomposition (a MAY) is not implemented; the deterministic
/// baseline emits only canonical, collision-prone denominations.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
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
        // `remaining` is what is left after removing every smallest-denomination unit,
        // so it is strictly below the smallest denomination and trivially in range.
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
        // 1.23456789 ZEC: 1 + 0.2 + 0.02 + 0.01, remainder 456_789 zat
        // (below MAX_RESIDUAL_VALUE = 0.01 ZEC).
        let d = decompose(Zatoshis::const_from_u64(123_456_789), &params());
        assert_eq!(
            values(d.outputs()),
            vec![
                100_000_000, // 1 ZEC
                20_000_000,  // 0.2
                2_000_000,   // 0.02
                1_000_000,   // 0.01
            ]
        );
        assert_eq!(u64::from(d.remainder()), 456_789);
    }

    /// Pins the worked examples in ZIP 318's amount-selection text. Greedy
    /// decomposition over the full `{1, 2, 5} × 10^k` ladder reproduces the
    /// ZIP's decimal digit expansion exactly, so these are conformance
    /// vectors shared with every other implementation of the rule.
    ///
    /// Test data: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
    #[test]
    fn zip_worked_examples() {
        // 123.45 ZEC -> [100, 20, 2, 1, 0.2, 0.2, 0.05]
        let d = decompose(Zatoshis::const_from_u64(12_345 * COIN / 100), &params());
        assert_eq!(
            values(d.outputs()),
            vec![
                100 * COIN,
                20 * COIN,
                2 * COIN,
                COIN,
                COIN / 5,
                COIN / 5,
                COIN / 20,
            ]
        );
        assert_eq!(d.remainder(), Zatoshis::ZERO);

        // 540 ZEC -> [500, 20, 20]
        let d = decompose(Zatoshis::const_from_u64(540 * COIN), &params());
        assert_eq!(values(d.outputs()), vec![500 * COIN, 20 * COIN, 20 * COIN]);
        assert_eq!(d.remainder(), Zatoshis::ZERO);

        // 25000 ZEC -> [10000, 10000, 5000]: value above the largest
        // denomination becomes repeated maximal-denomination parts.
        let d = decompose(Zatoshis::const_from_u64(25_000 * COIN), &params());
        assert_eq!(
            values(d.outputs()),
            vec![10_000 * COIN, 10_000 * COIN, 5_000 * COIN]
        );
        assert_eq!(d.remainder(), Zatoshis::ZERO);
    }

    /// Pins the ZIP's digit-expansion table: each digit expands greedily
    /// into `{5, 2, 1}` parts of its place value (9 yields `5, 2, 2`; 8
    /// yields `5, 2, 1`; 7 yields `5, 2`; 6 yields `5, 1`; 4 yields `2, 2`;
    /// 3 yields `2, 1`).
    ///
    /// Test data: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#amount-selection-canonical-quantization>
    #[test]
    fn zip_digit_expansion_table() {
        let expansions: [(u64, &[u64]); 9] = [
            (1, &[1]),
            (2, &[2]),
            (3, &[2, 1]),
            (4, &[2, 2]),
            (5, &[5]),
            (6, &[5, 1]),
            (7, &[5, 2]),
            (8, &[5, 2, 1]),
            (9, &[5, 2, 2]),
        ];
        for (digit, parts) in expansions {
            let d = decompose(Zatoshis::const_from_u64(digit * COIN), &params());
            let expected: Vec<u64> = parts.iter().map(|part| part * COIN).collect();
            assert_eq!(values(d.outputs()), expected, "digit {digit}");
            assert_eq!(d.remainder(), Zatoshis::ZERO);
        }
    }

    #[test]
    fn sub_denomination_value_is_all_remainder() {
        // 0.0009 ZEC is below the smallest denomination (0.01): no outputs.
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

        // The remainder is always below the smallest denomination, so folding it into
        // the fee costs at most one smallest-denomination unit.
        #[test]
        fn remainder_below_max_residual_value(value in 0u64..=MAX_MONEY) {
            let params = params();
            let d = decompose(Zatoshis::const_from_u64(value), &params);
            prop_assert!(u64::from(d.remainder()) < params.max_residual_value);
        }
    }
}
