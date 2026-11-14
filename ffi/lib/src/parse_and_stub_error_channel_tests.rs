use super::*;

#[test]
fn empty_address_travels_on_the_error_channel() {
    let error = parse_address(String::new())
        .expect_err("an empty address must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::InvalidInput(_)),
        "the failure must be the typed InvalidInput variant: {error}"
    );
}

#[test]
fn empty_ufvk_travels_on_the_error_channel() {
    let error = parse_ufvk(String::new())
        .expect_err("an empty viewkey must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::InvalidInput(_)),
        "the failure must be the typed InvalidInput variant: {error}"
    );
}

#[test]
fn unimplemented_option_stubs_fail_typed() {
    for (result, ffi) in [
        (set_option_wallet(), "set_option_wallet"),
        (get_option_wallet(), "get_option_wallet"),
    ] {
        let error = result.expect_err("an unimplemented endpoint must fail typed");
        assert!(
            matches!(error, ZingolibError::Wallet(_)),
            "{ffi} must fail with the typed Wallet variant: {error}"
        );
    }
}
