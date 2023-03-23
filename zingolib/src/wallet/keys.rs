//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use base58::{FromBase58, ToBase58};
use ripemd160::Digest;
use sha2::Sha256;
use std::io::{self, ErrorKind};
use zcash_client_backend::address;
use zcash_primitives::{
    legacy::TransparentAddress,
    sapling::PaymentAddress,
    zip32::{ChildIndex, DiversifiableFullViewingKey, ExtendedSpendingKey},
};
use zingoconfig::ZingoConfig;

#[cfg(feature = "integration_test")]
pub mod extended_transparent;
#[cfg(not(feature = "integration_test"))]
pub(crate) mod extended_transparent;
pub(crate) mod unified;

/// Sha256(Sha256(value))
pub fn double_sha256(payload: &[u8]) -> Vec<u8> {
    let h1 = Sha256::digest(&payload);
    let h2 = Sha256::digest(&h1);
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

/// A trait for converting base58check encoded values.
pub trait FromBase58Check {
    /// Convert a value of `self`, interpreted as base58check encoded data,
    /// into the tuple with version and payload as bytes vector.
    fn from_base58check(&self) -> io::Result<(u8, Vec<u8>)>;
}

impl FromBase58Check for str {
    fn from_base58check(&self) -> io::Result<(u8, Vec<u8>)> {
        let mut payload: Vec<u8> = match self.from_base58() {
            Ok(payload) => payload,
            Err(error) => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("{:?}", error),
                ))
            }
        };
        if payload.len() < 5 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("Invalid Checksum length"),
            ));
        }

        let checksum_index = payload.len() - 4;
        let provided_checksum = payload.split_off(checksum_index);
        let checksum = double_sha256(&payload)[..4].to_vec();
        if checksum != provided_checksum {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("Invalid Checksum"),
            ));
        }
        Ok((payload[0], payload[1..].to_vec()))
    }
}

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
            ChildIndex::Hardened(32),
            ChildIndex::Hardened(config.get_coin_type()),
            ChildIndex::Hardened(pos),
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

pub fn is_shielded_address(addr: &String, config: &ZingoConfig) -> bool {
    match address::RecipientAddress::decode(&config.chain, addr) {
        Some(address::RecipientAddress::Shielded(_))
        | Some(address::RecipientAddress::Unified(_)) => true,
        _ => false,
    }
}

/// STATIC METHODS
pub fn address_from_pubkeyhash(
    config: &ZingoConfig,
    taddr: Option<TransparentAddress>,
) -> Option<String> {
    match taddr {
        Some(TransparentAddress::PublicKey(hash)) => {
            Some(hash.to_base58check(&config.base58_pubkey_address(), &[]))
        }
        Some(TransparentAddress::Script(hash)) => {
            Some(hash.to_base58check(&config.base58_script_address(), &[]))
        }
        _ => None,
    }
}
