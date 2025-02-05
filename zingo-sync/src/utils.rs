use zcash_primitives::consensus::Parameters;

pub(crate) fn encode_orchard_receiver<P: Parameters>(
    parameters: &P,
    orchard_address: &orchard::Address,
) -> Result<String, ()> {
    Ok(zcash_address::unified::Encoding::encode(
        &<zcash_address::unified::Address as zcash_address::unified::Encoding>::try_from_items(
            vec![zcash_address::unified::Receiver::Orchard(
                orchard_address.to_raw_address_bytes(),
            )],
        )
        .unwrap(),
        &parameters.network_type(),
    ))
}
