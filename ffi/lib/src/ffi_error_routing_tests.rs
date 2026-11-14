use super::*;

#[test]
fn a_mixnet_refusal_maps_to_the_owned_mixnet_marker() {
    let mapped = ffi_error(LightClientError::MixnetNotReady(
        zingolib::mixnet::MixnetNotReady::Bootstrapping,
    ));
    assert!(
        matches!(&mapped, ZingolibError::Mixnet(_)),
        "a refusal must carry the owned mixnet marker: {mapped:?}"
    );
    assert!(mapped.to_string().starts_with("Error: mixnet:"));
}

#[test]
fn a_refused_vault_address_is_invalid_input_not_a_send_failure() {
    let malformed = match ZcashAddress::try_from_encoded("not an address") {
        Err(parse_error) => parse_error,
        Ok(_) => unreachable!("the string holds no address encoding"),
    };
    for inner in [
        WalletError::ParseError(malformed),
        WalletError::OpReturnRecipientNotTransparent,
    ] {
        let mapped = ffi_error(LightClientError::SendError(SendError::OpReturn(inner)));
        assert!(
            matches!(&mapped, ZingolibError::InvalidInput(_)),
            "a refused vault address is the caller's input, which the app never retries: {mapped:?}"
        );
        assert!(mapped.to_string().starts_with("Error: invalid input:"));
    }
}
