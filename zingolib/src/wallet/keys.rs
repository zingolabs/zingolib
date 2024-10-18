//! TODO: Add Mod Description Here!
//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use crate::config::{ChainType, ZingoConfig};
use base58::ToBase58;
use sapling_crypto::{
    zip32::{DiversifiableFullViewingKey, ExtendedSpendingKey},
    PaymentAddress,
};
use sha2::Sha256;
use zcash_client_backend::address;
use zcash_primitives::{
    consensus::NetworkConstants, legacy::TransparentAddress, zip32::ChildIndex,
};

pub mod legacy;
pub mod unified;

/// Sha256(Sha256(value))
pub fn double_sha256(payload: &[u8]) -> Vec<u8> {
    let h1 = <Sha256 as sha2::Digest>::digest(payload);
    let h2 = <Sha256 as sha2::Digest>::digest(h1.as_slice());
    h2.to_vec()
}

/// A trait for converting a [u8] to base58 encoded string.
pub trait ToBase58Check {
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

/// TODO: Add Doc Comment Here!
pub fn get_zaddr_from_bip39seed(
    config: &ZingoConfig,
    bip39_seed: &[u8],
    pos: u32,
) -> (
    ExtendedSpendingKey,
    DiversifiableFullViewingKey,
    PaymentAddress,
) {
    assert_eq!(bip39_seed.len(), 64);

    let extsk: ExtendedSpendingKey = ExtendedSpendingKey::from_path(
        &ExtendedSpendingKey::master(bip39_seed),
        &[
            ChildIndex::hardened(32),
            ChildIndex::hardened(config.chain.coin_type()),
            ChildIndex::hardened(pos),
        ],
    );
    let fvk = extsk.to_diversifiable_full_viewing_key();
    // Now we convert `ExtendedFullViewingKey` (EFVK) to `DiversifiableFullViewingKey` (DFVK).
    // DFVK is a subset of EFVK with same capabilities excluding the capability
    // of non-hardened key derivation. This is not a problem because Sapling non-hardened
    // key derivation has not been found useful in any real world scenario.
    //
    // On the other hand, only DFVK can be imported from Unified FVK. Degrading
    // EFVK to DFVK here enables us to keep one type of Sapling FVK across the wallet,
    // no matter whether the FVK was derived from SK or imported from UFVK.
    //
    // If the non-hardened key derivation is ever needed, we can recover EFVK easily
    // from Sapling extended spending key.
    let address = fvk.default_address().1;

    (extsk, fvk, address)
}

/// Checks if the address str is a valid zcash address
#[deprecated(note = "address strings are now immediately converted to valid addresses")]
pub fn is_shielded_address(addr: &str, chain: &ChainType) -> bool {
    matches!(
        address::Address::decode(chain, addr),
        Some(address::Address::Sapling(_)) | Some(address::Address::Unified(_))
    )
}

/// TODO: Add Doc Comment Here!
/// STATIC METHODS
pub fn address_from_pubkeyhash(config: &ZingoConfig, taddr: TransparentAddress) -> String {
    match taddr {
        TransparentAddress::PublicKeyHash(hash) => {
            hash.to_base58check(&config.chain.b58_pubkey_address_prefix(), &[])
        }
        TransparentAddress::ScriptHash(hash) => {
            hash.to_base58check(&config.chain.b58_script_address_prefix(), &[])
        }
    }
}
