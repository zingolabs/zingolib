use super::*;

fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
    let error = result.expect_err("an uninitialized client must fail typed");
    assert!(
        matches!(error, ZingolibError::LightclientNotInitialized),
        "{ffi} must fail with the typed uninitialized variant: {error}"
    );
}

#[test]
fn migration_status_fails_typed_without_a_client() {
    assert_uninitialized(migration_status(), "migration_status");
}
