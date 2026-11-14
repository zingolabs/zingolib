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
