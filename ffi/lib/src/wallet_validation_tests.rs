use super::*;

fn saved_wallet(chain_hint: &str) -> Vec<u8> {
    init_from_seed(
        "hospital museum valve antique skate museum \
             unfold vocal weird milk scale social vessel identify \
             crowd hospital control album rib bulb path oven civil tank"
            .to_string(),
        2_000_000,
        test_connection("", chain_hint, "Medium"),
    )
    .expect("offline init from seed");
    save_wallet_bytes()
        .expect("save after init")
        .expect("an initialized wallet always yields bytes")
}

#[test]
fn a_saved_wallet_validates_and_every_truncation_fails() {
    let _serial = lock_discipline_tests::serialized();
    let bytes = saved_wallet("main");
    validate_wallet_bytes(bytes.clone()).expect("the saved wallet must validate");

    for cut in [0, 8, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            matches!(
                validate_wallet_bytes(bytes[..cut].to_vec()),
                Err(ZingolibError::Read(_))
            ),
            "the wallet cut to {cut} of {} bytes must fail validation",
            bytes.len()
        );
    }

    validate_wallet_bytes(saved_wallet("test")).expect("the testnet wallet must validate");
}

#[test]
fn bytes_that_pass_the_header_heuristic_still_fail_validation() {
    let mut bytes = 42u64.to_le_bytes().to_vec();
    bytes.extend([0x07; 400]);
    assert!(matches!(
        validate_wallet_bytes(bytes),
        Err(ZingolibError::Read(_))
    ));
}
