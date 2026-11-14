use super::*;

const RSEED_BEFORE_ZIP212: u8 = 1;
const RSEED_UNKNOWN: u8 = 3;

#[test]
fn a_non_canonical_orchard_nullifier_is_its_own_decode_error() {
    assert!(matches!(
        orchard_nullifier(&[u8::MAX; _]),
        Err(DecodeError::OrchardNullifier)
    ));
}

#[test]
fn a_canonical_orchard_nullifier_round_trips() {
    let canonical = [u8::MIN; _];
    assert_eq!(
        orchard_nullifier(&canonical)
            .ok()
            .map(|nullifier| nullifier.to_bytes()),
        Some(canonical)
    );
}

#[test]
fn a_decode_error_reaches_the_caller_as_invalid_data() {
    assert_eq!(
        io::Error::from(DecodeError::OrchardNullifier).kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn each_rseed_decode_error_is_its_own_variant() {
    assert!(matches!(
        sapling_rseed(RSEED_BEFORE_ZIP212, [u8::MAX; _]),
        Err(DecodeError::SaplingRseedScalar)
    ));
    assert!(matches!(
        sapling_rseed(RSEED_UNKNOWN, [u8::MIN; _]),
        Err(DecodeError::SaplingRseedType(found)) if found == RSEED_UNKNOWN
    ));
}

#[test]
fn memo_bytes_longer_than_a_memo_are_a_memo_decode_error() {
    assert!(matches!(
        memo(&[u8::MIN; MEMO_SIZE + 1]),
        Err(DecodeError::Memo(_))
    ));
}

#[test]
fn a_memo_error_prints_its_own_layer_and_keeps_the_cause_as_its_source() {
    let error = memo(&[u8::MIN; MEMO_SIZE + 1]).err();
    assert_eq!(
        error.as_ref().map(ToString::to_string).as_deref(),
        Some("Couldn't create memo")
    );
    assert!(
        error
            .as_ref()
            .and_then(std::error::Error::source)
            .is_some_and(|source| source.is::<zcash_protocol::memo::Error>())
    );
}
