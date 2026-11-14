use super::*;

fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
    let error = result.expect_err("an uninitialized client must fail typed");
    assert!(
        matches!(error, ZingolibError::LightclientNotInitialized),
        "{ffi} must fail with the typed uninitialized variant: {error}"
    );
}

#[test]
fn run_sync_fails_typed_without_a_client() {
    assert_uninitialized(run_sync(), "run_sync");
}

#[test]
fn pause_sync_fails_typed_without_a_client() {
    assert_uninitialized(pause_sync(), "pause_sync");
}

#[test]
fn status_sync_fails_typed_without_a_client() {
    assert_uninitialized(status_sync(), "status_sync");
}

#[test]
fn poll_sync_fails_typed_without_a_client() {
    assert_uninitialized(poll_sync(), "poll_sync");
}

#[test]
fn run_rescan_fails_typed_without_a_client() {
    assert_uninitialized(run_rescan(), "run_rescan");
}
