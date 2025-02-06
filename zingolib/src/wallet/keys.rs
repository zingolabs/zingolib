//! TODO: Add Mod Description Here!
//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use crate::config::ZingoConfig;
use base58::ToBase58;
use sapling_crypto::{
    zip32::{DiversifiableFullViewingKey, ExtendedSpendingKey},
    PaymentAddress,
};
use sha2::Sha256;
use unified::ReceiverSelection;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::{consensus::NetworkConstants, zip32::ChildIndex};

use super::{error::KeyError, LightWallet};

pub mod legacy;
pub mod unified;

impl LightWallet {
    /// Returns a new unified address for the given `receivers`.
    /// Also adds this new unified address to the wallet.
    pub fn generate_unified_address(
        &self,
        receivers: ReceiverSelection,
    ) -> Result<UnifiedAddress, KeyError> {
        let unified_address = self.unified_key_store.generate_unified_address(
            self.unified_addresses.len() as u32,
            receivers,
            false,
        )?;

        self.unified_addresses.push(unified_address.clone());

        Ok(unified_address)
    }
}

// TODO: zingo2, remove?
/// Sha256(Sha256(value))
pub fn double_sha256(payload: &[u8]) -> Vec<u8> {
    let h1 = <Sha256 as sha2::Digest>::digest(payload);
    let h2 = <Sha256 as sha2::Digest>::digest(h1.as_slice());
    h2.to_vec()
}

// TODO: zingo2, remove?
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
