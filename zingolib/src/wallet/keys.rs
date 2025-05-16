//! TODO: Add Mod Description Here!
//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use crate::config::ZingoConfig;
use pepper_sync::{
    keys::transparent::{self, TransparentAddressId, TransparentScope},
    wallet::KeyIdInterface,
};
use sapling_crypto::{
    PaymentAddress,
    zip32::{DiversifiableFullViewingKey, ExtendedSpendingKey},
};
use unified::{ReceiverSelection, UnifiedAddressId};
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::{
    consensus::NetworkConstants,
    legacy::{TransparentAddress, keys::NonHardenedChildIndex},
    zip32::ChildIndex,
};

use super::{LightWallet, error::KeyError};

pub mod legacy;
pub mod unified;

impl LightWallet {
    /// Returns a new unified address for the given `receivers` and `account_id`.
    /// Also adds this new unified address to the wallet.
    /// If the unified address contains a transparent receiver, this is also added to transparent addresses.
    pub fn generate_unified_address(
        &mut self,
        receivers: ReceiverSelection,
        account_id: zip32::AccountId,
    ) -> Result<UnifiedAddress, KeyError> {
        let unified_address_index = self
            .unified_addresses
            .keys()
            .filter(|&address_id| address_id.account_id == account_id)
            .count() as u32;
        let unified_address = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .generate_unified_address(unified_address_index, receivers, false)?;

        if let Some(transparent_address) = unified_address.transparent() {
            self.transparent_addresses.insert(
                TransparentAddressId::new(
                    account_id,
                    TransparentScope::External,
                    NonHardenedChildIndex::from_index(unified_address_index)
                        .expect("all non-hardened addresses in use!"),
                ),
                transparent::encode_address(&self.network, *transparent_address),
            );
        }

        self.unified_addresses.insert(
            UnifiedAddressId {
                account_id,
                address_index: unified_address_index,
            },
            unified_address.clone(),
        );
        self.save_required = true;

        Ok(unified_address)
    }

    /// Generates 'n' new refund addresses and adds them to the wallet.
    pub fn generate_refund_addresses(
        &mut self,
        n: usize,
        account_id: zip32::AccountId,
    ) -> Result<Vec<(TransparentAddressId, TransparentAddress)>, KeyError> {
        let refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| {
                address_id.scope() == TransparentScope::Refund
                    && address_id.account_id() == account_id
            })
            .count();

        let refund_addresses = (refund_address_count..(refund_address_count + n))
            .map(|address_index| {
                let transparent_address_id = TransparentAddressId::new(
                    account_id,
                    TransparentScope::Refund,
                    NonHardenedChildIndex::from_index(address_index as u32)
                        .expect("all non-hardened addresses in use!"),
                );
                let refund_address = self
                    .unified_key_store
                    .get(&account_id)
                    .ok_or(KeyError::NoAccountKeys)?
                    .generate_transparent_address(
                        address_index as u32,
                        TransparentScope::Refund,
                        false,
                    )?;

                self.transparent_addresses.insert(
                    transparent_address_id,
                    transparent::encode_address(&self.network, refund_address),
                );

                Ok((transparent_address_id, refund_address))
            })
            .collect::<Result<Vec<(TransparentAddressId, TransparentAddress)>, KeyError>>()?;
        self.save_required = true;

        Ok(refund_addresses)
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
