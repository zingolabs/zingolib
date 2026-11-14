use super::*;
use crate::config::ChainType;
use proptest::prelude::*;
use zcash_protocol::value::COIN;
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
    // 1.23456789 ZEC: 1 + 0.2 + 0.02 + 0.01, remainder 456_789 zat.
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

#[test]
fn value_above_largest_denomination_uses_many_top_notes() {
    // 25 000 ZEC → 2×10 000 + 5000, no remainder.
    let d = decompose(Zatoshis::const_from_u64(25_000 * COIN), &params());
    assert_eq!(
        values(d.outputs()),
        vec![10_000 * COIN, 10_000 * COIN, 5_000 * COIN]
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

    // The remainder is always below the smallest denomination, so folding it into
    // the fee costs at most one smallest-denomination unit.
    #[test]
    fn remainder_below_max_residual_value(value in 0u64..=MAX_MONEY) {
        let params = params();
        let d = decompose(Zatoshis::const_from_u64(value), &params);
        prop_assert!(u64::from(d.remainder()) < params.max_residual_value);
    }
}
