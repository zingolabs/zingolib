use super::*;

#[test]
fn save_failure_travels_on_the_error_channel() {
    let error = map_wallet_save(Err(std::io::Error::other("disk full")))
        .expect_err("a failed save must be typed, not prose in the data channel");
    assert!(
        matches!(error, ZingolibError::Save(_)),
        "the failure must be the typed Save variant: {error}"
    );
}

#[test]
fn wallet_bytes_cross_the_data_channel_verbatim() {
    let wallet_bytes = vec![0xde, 0xad, 0xbe, 0xef];
    let crossed = map_wallet_save(Ok(Some(wallet_bytes.clone())))
        .expect("wallet bytes travel on the data channel");
    assert_eq!(crossed, Some(wallet_bytes));
}

#[test]
fn no_save_needed_crosses_as_null() {
    let crossed = map_wallet_save(Ok(None)).expect("no-save is not a failure");
    assert_eq!(crossed, None);
}
