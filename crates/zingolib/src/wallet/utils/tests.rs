use super::*;

/// HYPOTHESIS: the embedded Sapling parameters are present whole, so
/// loading them is infallible. Falsified if either blob is empty.
#[test]
fn embedded_sapling_params_load_whole() {
    assert!(
        !crate::SaplingParams::OUTPUT.is_empty(),
        "output params are embedded"
    );
    assert!(
        !crate::SaplingParams::SPEND.is_empty(),
        "spend params are embedded"
    );
}

#[test]
fn test_memo_bytes_from_string_plain_text() {
    let result = memo_bytes_from_string("Hello World".to_string());
    assert!(result.is_ok());
}

#[test]
fn test_memo_bytes_from_string_valid_hex() {
    // Valid hex should be decoded
    let result = memo_bytes_from_string("0x48656c6c6f".to_string());
    assert!(result.is_ok());
}

#[test]
fn test_memo_bytes_from_string_ethereum_address() {
    // Ethereum address should be treated as plain text
    let eth_address = "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb";
    let result = memo_bytes_from_string(eth_address.to_string());
    assert!(result.is_ok());
    // Verify it's stored as the full string (43 bytes), not decoded hex (20 bytes)
    let memo_bytes = result.unwrap();
    assert_eq!(memo_bytes.as_slice().len(), eth_address.len());
}

#[test]
fn test_memo_bytes_from_string_invalid_hex() {
    // Invalid hex should fall back to plain text
    let result = memo_bytes_from_string("0xZZZZ".to_string());
    assert!(result.is_ok());
}

#[test]
fn test_memo_bytes_from_string_too_long() {
    // String longer than 512 bytes should fail
    let long_string = "a".repeat(513);
    let result = memo_bytes_from_string(long_string);
    assert!(result.is_err());
}
