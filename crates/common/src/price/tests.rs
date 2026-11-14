use super::*;

/// The reader is given a serialized version above the one the type writes, followed by a price list the current
/// layout reads.
#[test]
fn price_list_read_refuses_a_serialized_version_above_its_own() {
    let mut bytes = Vec::new();
    PriceList::new().write(&mut bytes).unwrap();
    PriceList::read(bytes.as_slice()).expect("the current version is read");

    bytes[0] = PriceList::serialized_version() + 1;

    let error = PriceList::read(bytes.as_slice())
        .map(drop)
        .expect_err("a version above the current version is refused");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}
