use super::test_vectors as zingomemo_vectors;
use super::*;
use test_vectors::TestVector;
use zcash_protocol::consensus::{BlockHeight, MAIN_NETWORK};

fn get_serialiazed_ua(test_vector: &TestVector) -> (UnifiedAddress, Vec<u8>) {
    let zcash_keys::address::Address::Unified(ua) =
        zcash_keys::address::Address::decode(&MAIN_NETWORK, test_vector.unified_addr).unwrap()
    else {
        panic!("Couldn't decode test_vector UA")
    };
    let mut serialized_ua = Vec::new();
    write_unified_address_to_raw_encoding(&MAIN_NETWORK, &ua, &mut serialized_ua).unwrap();
    (*ua, serialized_ua)
}
/// A Revision 0 address is written as a version 1 memo, the framing every
/// reader knows, with an empty refund address list.
#[test]
fn a_revision_0_address_round_trips_through_version_1() {
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, _) = get_serialiazed_ua(test_vector);
        let packed = create_wallet_internal_memo(&MAIN_NETWORK, std::slice::from_ref(&ua))
            .expect("To create version 1 bytes");
        let ParsedMemo::Version1 {
            uas,
            rejection_address_indexes,
        } = parse_zingo_memo(packed.bytes).expect("To succeed in parse.")
        else {
            panic!("a Revision 0 address is written as version 1")
        };
        assert_eq!(uas, [ua]);
        assert!(rejection_address_indexes.is_empty());
    }
}
#[test]
fn round_trip_ser_deser() {
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, serialized_ua) = get_serialiazed_ua(test_vector);
        assert_eq!(
            ua,
            read_unified_address_from_raw_encoding(&*serialized_ua).unwrap()
        );
    }
}

/// The same receivers as `ua`, with expiry metadata that only ZIP 316
/// Revision 2 can carry.
fn with_expiry(ua: &UnifiedAddress) -> UnifiedAddress {
    UnifiedAddress::from_receivers(
        ua.orchard().copied(),
        ua.sapling().cloned(),
        ua.transparent().copied(),
        Some(BlockHeight::from_u32(3_000_000)),
        Some(1_900_000_000),
    )
    .expect("the vector has a receiver")
}

/// A version 2 memo reads back an expiring address with its metadata, and
/// a transparent-only address, which Revision 0 cannot carry at all.
#[test]
fn version_2_records_revision_2_addresses_exactly() {
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, _) = get_serialiazed_ua(test_vector);
        let mut uas = vec![with_expiry(&ua)];
        if let Some(transparent) = ua.transparent().copied() {
            uas.push(
                UnifiedAddress::from_receivers(None, None, Some(transparent), None, None)
                    .expect("a transparent receiver suffices"),
            );
        }
        let packed =
            create_wallet_internal_memo(&MAIN_NETWORK, &uas).expect("to create version 2 bytes");
        assert_eq!(packed.recorded, uas.len());
        let ParsedMemo::Version2 { uas: parsed } =
            parse_zingo_memo(packed.bytes).expect("to succeed in parse")
        else {
            panic!("a version 2 memo parses as version 2")
        };
        assert_eq!(parsed, uas);
    }
}

/// A Revision 2 address among Revision 0 ones lifts the memo to version 2.
#[test]
fn a_revision_2_address_lifts_the_memo_to_version_2() {
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, _) = get_serialiazed_ua(test_vector);
        let expiring = with_expiry(&ua);
        let packed = create_wallet_internal_memo(&MAIN_NETWORK, &[ua.clone(), expiring]).unwrap();
        assert!(matches!(
            parse_zingo_memo(packed.bytes).unwrap(),
            ParsedMemo::Version2 { .. }
        ));
    }
}

/// Addresses that outgrow the memo field are left off the end rather than
/// failing the memo: the field records the longest prefix that fits, and
/// the count says how many.
#[test]
fn an_overflowing_list_records_the_prefix_that_fits() {
    // A P2PKH receiver is the smallest an address can be, so this many
    // addresses overflow the field whatever receivers the vector carries.
    const SMALLEST_ADDRESS_LEN: usize = 20;
    let too_many = MEMO_LEN / SMALLEST_ADDRESS_LEN + 1;
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, _) = get_serialiazed_ua(test_vector);
        let uas = vec![with_expiry(&ua); too_many];
        let packed = create_wallet_internal_memo(&MAIN_NETWORK, &uas).unwrap();
        assert!(packed.recorded > 0);
        assert!(packed.recorded < too_many);
        assert_eq!(
            parse_zingo_memo(packed.bytes)
                .unwrap()
                .into_unified_addresses(),
            uas[..packed.recorded]
        );
        let one_more =
            create_wallet_internal_memo(&MAIN_NETWORK, &uas[..packed.recorded + 1]).unwrap();
        assert_eq!(one_more.recorded, packed.recorded);
    }
}

/// The raw encoding, which versions 0 and 1 carry, refuses an address it
/// would record incompletely instead of dropping its metadata.
#[test]
fn the_raw_encoding_refuses_a_revision_2_address() {
    for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
        let (ua, _) = get_serialiazed_ua(test_vector);
        let error = write_unified_address_to_raw_encoding(
            &MAIN_NETWORK,
            &with_expiry(&ua),
            &mut Vec::new(),
        )
        .expect_err("an expiring address has no raw encoding");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
