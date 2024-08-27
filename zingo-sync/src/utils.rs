use base58::ToBase58;

use zcash_primitives::{
    consensus::{NetworkConstants, Parameters},
    legacy::TransparentAddress,
};

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
pub(crate) fn address_from_pubkeyhash<P: NetworkConstants>(
    parameters: &P,
    transparent_address: &TransparentAddress,
) -> String {
    match transparent_address {
        TransparentAddress::PublicKeyHash(hash) => {
            hash.to_base58check(&parameters.b58_pubkey_address_prefix(), &[])
        }
        TransparentAddress::ScriptHash(hash) => {
            hash.to_base58check(&parameters.b58_script_address_prefix(), &[])
        }
    }
}
/// A trait for converting a [u8] to base58 encoded string.
trait ToBase58Check {
    /// Converts a value of `self` to a base58 value, returning the owned string.
    /// The version is a coin-specific prefix that is added.
    /// The suffix is any bytes that we want to add at the end (like the "iscompressed" flag for
    /// Secret key encoding)
    fn to_base58check(&self, version: &[u8], suffix: &[u8]) -> String;
}
impl ToBase58Check for [u8] {
    fn to_base58check(&self, version: &[u8], suffix: &[u8]) -> String {
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(version);
        payload.extend_from_slice(self);
        payload.extend_from_slice(suffix);

        let checksum = double_sha256(&payload);
        payload.append(&mut checksum[..4].to_vec());
        payload.to_base58()
    }
}
/// Sha256(Sha256(value))
fn double_sha256(payload: &[u8]) -> Vec<u8> {
    let h1 = <sha2::Sha256 as sha2::Digest>::digest(payload);
    let h2 = <sha2::Sha256 as sha2::Digest>::digest(&h1);
    h2.to_vec()
}
