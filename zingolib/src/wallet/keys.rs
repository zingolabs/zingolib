//! [crate::wallet::LightWallet] methods associated with keys and address derivation.

use pepper_sync::{
    keys::{
        decode_address,
        transparent::{self, TransparentAddressId, TransparentScope},
    },
    wallet::KeyIdInterface,
};
use unified::{ReceiverSelection, UnifiedAddressId};
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::legacy::{TransparentAddress, keys::NonHardenedChildIndex};
use zip32::DiversifierIndex;

use super::{LightWallet, error::KeyError};

pub mod legacy;
pub mod unified;

impl LightWallet {
    /// Returns a new unified address for the given `receivers` and `account_id`.
    /// Also adds this new unified address to the wallet.
    ///
    /// Although supported, it is not recommended to include transparent receivers in unified addresses.
    /// If the unified address contains a transparent receiver, this is also added to the wallet's transparent addresses.
    pub fn generate_unified_address(
        &mut self,
        receivers: ReceiverSelection,
        account_id: zip32::AccountId,
    ) -> Result<(UnifiedAddressId, UnifiedAddress), KeyError> {
        let address_id = UnifiedAddressId {
            account_id,
            address_index: self
                .unified_addresses
                .keys()
                .filter(|&address_id| address_id.account_id == account_id)
                .map(|&address_id| address_id.address_index)
                .max()
                .map_or(0, |address_id| address_id + 1),
        };
        let unified_address = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .generate_unified_address(address_id.address_index, receivers, false)?;

        if let Some(transparent_address) = unified_address.transparent() {
            self.transparent_addresses.insert(
                TransparentAddressId::new(
                    account_id,
                    TransparentScope::External,
                    NonHardenedChildIndex::from_index(address_id.address_index)
                        .ok_or(KeyError::InvalidNonHardenedChildIndex)?,
                ),
                transparent::encode_address(&self.network, *transparent_address),
            );
        }
        self.unified_addresses
            .insert(address_id, unified_address.clone());
        self.save_required = true;

        Ok((address_id, unified_address))
    }

    /// Generates a new transparent address of `external` scope for the given `account_id`.
    /// The new address is added to the wallet and returned.
    pub fn generate_transparent_address(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<(TransparentAddressId, TransparentAddress), KeyError> {
        let address_index = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| {
                address_id.scope() == TransparentScope::External
                    && address_id.account_id() == account_id
            })
            .map(|&address_id| address_id.address_index())
            .max()
            .map_or(Ok(NonHardenedChildIndex::ZERO), |address_index| {
                address_index
                    .next()
                    .ok_or(KeyError::InvalidNonHardenedChildIndex)
            })?;
        let address_id =
            TransparentAddressId::new(account_id, TransparentScope::External, address_index);
        let external_address = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .generate_transparent_address(address_id.address_index(), address_id.scope(), false)?;

        self.transparent_addresses.insert(
            address_id,
            transparent::encode_address(&self.network, external_address),
        );
        self.save_required = true;

        Ok((address_id, external_address))
    }

    /// Generates 'n' new transparent addresses of `refund` (ephemeral) scope for the given `account_id`.
    /// The new addresses are added to the wallet and returned.
    pub fn generate_refund_addresses(
        &mut self,
        n: usize,
        account_id: zip32::AccountId,
    ) -> Result<Vec<(TransparentAddressId, TransparentAddress)>, KeyError> {
        let first_index = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| {
                address_id.scope() == TransparentScope::Refund
                    && address_id.account_id() == account_id
            })
            .map(|&address_id| address_id.address_index())
            .max()
            .map_or(Ok(NonHardenedChildIndex::ZERO), |address_index| {
                address_index
                    .next()
                    .ok_or(KeyError::InvalidNonHardenedChildIndex)
            })?
            .index() as usize;

        let refund_addresses = (first_index..(first_index + n))
            .map(|address_index| {
                let address_id = TransparentAddressId::new(
                    account_id,
                    TransparentScope::Refund,
                    NonHardenedChildIndex::from_index(address_index as u32)
                        .ok_or(KeyError::InvalidNonHardenedChildIndex)?,
                );
                let refund_address = self
                    .unified_key_store
                    .get(&account_id)
                    .ok_or(KeyError::NoAccountKeys)?
                    .generate_transparent_address(
                        address_id.address_index(),
                        address_id.scope(),
                        false,
                    )?;

                self.transparent_addresses.insert(
                    address_id,
                    transparent::encode_address(&self.network, refund_address),
                );

                Ok((address_id, refund_address))
            })
            .collect::<Result<Vec<(TransparentAddressId, TransparentAddress)>, KeyError>>()?;
        self.save_required = true;

        Ok(refund_addresses)
    }

    /// Determines whether the `encoded_address` is derived by the wallet's keys.
    ///
    /// Fails to detect internal sapling addresses.
    /// https://github.com/zcash/sapling-crypto/issues/160.
    pub fn is_wallet_address(&self, encoded_address: &str) -> Result<bool, KeyError> {
        Ok(match decode_address(&self.network, encoded_address)? {
            zcash_keys::address::Address::Transparent(address) => {
                self.is_transparent_wallet_address(&address).is_some()
            }
            zcash_keys::address::Address::Sapling(address) => {
                self.is_sapling_external_wallet_address(&address).is_some()
            }
            zcash_keys::address::Address::Unified(address) => {
                address
                    .transparent()
                    .is_some_and(|addr| self.is_transparent_wallet_address(addr).is_some())
                    || address
                        .sapling()
                        .is_some_and(|addr| self.is_sapling_external_wallet_address(addr).is_some())
                    || address
                        .orchard()
                        .is_some_and(|addr| self.is_orchard_wallet_address(addr).is_some())
            }
            zcash_keys::address::Address::Tex(_) => false,
        })
    }

    /// Returns the address identifier if the given `address` is one of the wallet's derived addresses.
    pub fn is_transparent_wallet_address(
        &self,
        address: &TransparentAddress,
    ) -> Option<TransparentAddressId> {
        let encoded_address = transparent::encode_address(&self.network, *address);

        self.transparent_addresses
            .iter()
            .find(|(_, wallet_address)| **wallet_address == encoded_address)
            .map(|(address_id, _)| *address_id)
    }

    /// Returns the account id and diversifier index if the given `address` is derived from the wallet's sapling FVKs. External scope only.
    pub fn is_sapling_external_wallet_address(
        &self,
        address: &sapling_crypto::PaymentAddress,
    ) -> Option<(zip32::AccountId, DiversifierIndex)> {
        for (account_id, unified_key) in self.unified_key_store.iter() {
            if let Some((diversifier_index, _)) =
                sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(unified_key)
                    .ok()
                    .and_then(|fvk| fvk.decrypt_diversifier(address))
            {
                return Some((*account_id, diversifier_index));
            }
        }

        None
    }

    /// Returns the account id and diversifier index if the given `address` is derived from the wallet's orchard FVKs.
    pub fn is_orchard_wallet_address(
        &self,
        address: &orchard::Address,
    ) -> Option<(zip32::AccountId, zip32::Scope, DiversifierIndex)> {
        for (account_id, unified_key) in self.unified_key_store.iter() {
            let Ok(fvk) = orchard::keys::FullViewingKey::try_from(unified_key) else {
                continue;
            };
            for scope in [zip32::Scope::External, zip32::Scope::Internal] {
                if let Some(diversifier_index) = fvk.to_ivk(scope).diversifier_index(address) {
                    return Some((*account_id, scope, diversifier_index));
                }
            }
        }

        None
    }
}
