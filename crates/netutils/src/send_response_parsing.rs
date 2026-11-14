use super::*;

#[test]
fn acceptance_unquotes_the_txid() {
    assert_eq!(
        parse_send_response(0, "\"deadbeef\"".to_string()),
        Ok("deadbeef".to_string())
    );
}

#[test]
fn acceptance_passes_a_bare_txid_through() {
    assert_eq!(
        parse_send_response(0, "deadbeef".to_string()),
        Ok("deadbeef".to_string())
    );
}

#[test]
fn rejection_carries_the_code_and_the_server_message() {
    assert_eq!(
        parse_send_response(-25, "failed to validate".to_string()),
        Err(SendRejection {
            code: -25,
            message: "failed to validate".to_string()
        })
    );
}

/// A one-character `"` message satisfies both `starts_with('"')` and
/// `ends_with('"')` (they see the same byte), so unquoting must not
/// slice `[1..0]`. Pins the review's finding 4.
#[test]
fn a_lone_quote_acceptance_does_not_panic() {
    assert_eq!(
        parse_send_response(0, "\"".to_string()),
        Ok("\"".to_string())
    );
}
