use super::*;

/// The reader is given a serialized version above the one the type writes, followed by a status and block height
/// the current layout reads.
#[test]
fn read_refuses_a_serialized_version_above_its_own() {
    let mut bytes = Vec::new();
    ConfirmationStatus::Confirmed(BlockHeight::from_u32(1))
        .write(&mut bytes)
        .unwrap();
    ConfirmationStatus::read(bytes.as_slice()).expect("the current version is read");

    bytes[0] = ConfirmationStatus::serialized_version() + 1;

    let error = ConfirmationStatus::read(bytes.as_slice())
        .expect_err("a version above the current version is refused");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("newer build"),
        "the error must name the cause: {error}"
    );
}
