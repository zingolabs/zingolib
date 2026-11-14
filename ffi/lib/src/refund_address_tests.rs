use super::*;
use zcash_transparent::keys::NonHardenedChildIndex;

#[test]
fn a_reserved_refund_address_reports_the_refund_scope() {
    let id = transparent::TransparentAddressId::new(
        AccountId::ZERO,
        transparent::TransparentScope::Refund,
        NonHardenedChildIndex::ZERO,
    );
    let encoded_address = transparent::encode_address(
        &zingolib::config::ChainType::Mainnet,
        TransparentAddress::PublicKeyHash([7; 20]),
    );
    let report = NewTransparentAddress::Created {
        id,
        encoded_address,
    }
    .report()
    .expect("a created address always reports");
    let parsed = json::parse(&report).expect("the report is JSON");
    assert_eq!(parsed["account"].as_u32(), Some(0));
    assert_eq!(parsed["address_index"].as_u32(), Some(0));
    assert_eq!(parsed["scope"].as_str(), Some("refund"));
    assert!(
        parsed["encoded_address"]
            .as_str()
            .is_some_and(|address| address.starts_with("t1"))
    );
}
