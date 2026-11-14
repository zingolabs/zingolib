use super::*;

fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
    let error = result.expect_err("an uninitialized client must fail typed");
    assert!(
        matches!(error, ZingolibError::LightclientNotInitialized),
        "{ffi} must fail with the typed uninitialized variant: {error}"
    );
}

#[test]
fn get_balance_fails_typed_without_a_client() {
    assert_uninitialized(get_balance(), "get_balance");
}

#[test]
fn get_spendable_balance_total_fails_typed_without_a_client() {
    assert_uninitialized(get_spendable_balance_total(), "get_spendable_balance_total");
}

#[test]
fn get_value_transfers_fails_typed_without_a_client() {
    assert_uninitialized(get_value_transfers(), "get_value_transfers");
}

#[test]
fn get_messages_fails_typed_without_a_client() {
    assert_uninitialized(get_messages(String::new()), "get_messages");
}

#[test]
fn invalid_server_uri_travels_on_the_error_channel() {
    let error = get_latest_block_server("http://an invalid uri with spaces".to_string())
        .expect_err("an unparseable server uri must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Read(_)),
        "the failure must be the typed Read variant: {error}"
    );
}
