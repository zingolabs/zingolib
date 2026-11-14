use super::*;

#[test]
fn invalid_server_uri_travels_on_the_error_channel() {
    let _serial = lock_discipline_tests::serialized();
    let error = init_new(
        test_connection("http://an invalid uri with spaces", "main", "Medium"),
        0,
    )
    .expect_err("an invalid lightwalletd uri must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Init(_)),
        "the failure must be the typed Init variant: {error}"
    );
}

#[test]
fn invalid_performance_level_travels_on_the_error_channel() {
    let error = init_from_seed(
        "unvalidated at this point".to_string(),
        1,
        test_connection("", "main", "NotALevel"),
    )
    .expect_err("an invalid performance level must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Init(_)),
        "the failure must be the typed Init variant: {error}"
    );
}

#[test]
fn ufvk_restore_failure_travels_on_the_error_channel() {
    let error = init_from_ufvk(
        "unvalidated at this point".to_string(),
        1,
        test_connection("", "main", "NotALevel"),
    )
    .expect_err("an invalid performance level must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Init(_)),
        "the failure must be the typed Init variant: {error}"
    );
}

#[test]
fn reset_clears_the_pending_indexer_uri() {
    if let Ok(mut pending) = PENDING_INDEXER_URI.write() {
        *pending = Some(http::Uri::from_static("https://example.com:9067"));
    }
    reset_lightclient();
    let pending = PENDING_INDEXER_URI.read().expect("pending uri lock");
    assert!(
        pending.is_none(),
        "a client teardown must not leave a stale pending URI behind"
    );
}

#[test]
fn unreadable_wallet_bytes_travel_on_the_error_channel() {
    let _serial = lock_discipline_tests::serialized();
    let error = init_from_bytes(
        b"!!!not-a-wallet!!!".to_vec(),
        test_connection("", "main", "Medium"),
    )
    .expect_err("unreadable wallet bytes must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Init(_)),
        "the failure must be the typed Init variant: {error}"
    );
    assert!(
        !error.to_string().contains("not-a-wallet"),
        "the failure must not embed the payload it could not read: {error}"
    );
}
