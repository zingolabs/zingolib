use super::*;

#[test]
fn a_hash_round_trips_through_hex_and_back() {
    let hash: [u8; 32] = core::array::from_fn(|i| i as u8);
    let hex = hash32_to_hex(&hash);
    assert_eq!(hex.len(), 64);
    assert_eq!(hex_to_hash32(&hex), Some(hash));
}

#[test]
fn uppercase_hex_decodes_to_the_same_hash() {
    let hash = [0xabu8; 32];
    let hex = hash32_to_hex(&hash).to_uppercase();
    assert_eq!(hex_to_hash32(&hex), Some(hash));
}

#[test]
fn anything_but_64_hex_digits_is_rejected() {
    let valid = hash32_to_hex(&[0u8; 32]);
    let rejected = [
        String::new(),
        valid[..62].to_string(),
        valid[..63].to_string(),
        format!("{valid}00"),
        format!("zz{}", &valid[2..]),
        format!("+a{}", &valid[2..]),
        format!("é{}", &valid[2..]),
    ];
    for input in rejected {
        assert_eq!(
            hex_to_hash32(&input),
            None,
            "{input:?} must be rejected, not coerced into a consent hash"
        );
    }
}

#[test]
fn plan_fails_typed_without_a_client() {
    let error = PrivateMigrationDriver::new()
        .plan()
        .expect_err("no test initializes the global lightclient, so plan cannot succeed");
    assert!(
        matches!(error, ZingolibError::LightclientNotInitialized),
        "plan must fail with the typed uninitialized variant: {error}"
    );
}

#[test]
fn a_bad_plan_hash_is_refused_before_the_lock_is_taken() {
    let error = PrivateMigrationDriver::new()
        .start("zz".to_string(), None)
        .expect_err("two non-hex characters are not a consent hash");
    assert!(matches!(error, ZingolibError::InvalidInput(_)), "{error}");
}
