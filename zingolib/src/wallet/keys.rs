//! [crate::wallet::LightWallet] methods associated with keys and addresses.

use pepper_sync::{
    keys::transparent::{self, TransparentAddressId, TransparentScope},
    wallet::KeyIdInterface,
};
use unified::{ReceiverSelection, UnifiedAddressId};
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::legacy::{TransparentAddress, keys::NonHardenedChildIndex};

use super::{LightWallet, error::KeyError};

pub mod legacy;
pub mod unified;

impl LightWallet {
    /// Returns unified addresses in a JSON array.
    pub fn unified_addresses(&self) -> json::JsonValue {
        json::JsonValue::Array(
            self.unified_addresses
                .iter()
                .map(|(id, unified_address)| {
                    json::object! {
                        "account" => u32::from(id.account_id),
                        "address_index" => id.address_index,
                        "has_orchard" => unified_address.has_orchard(),
                        "has_sapling" => unified_address.has_sapling(),
                        "has_transparent" => unified_address.has_transparent(),
                        "encoded_address" => unified_address.encode(&self.network),
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Returns transparent addresses in a JSON array.
    pub fn transparent_addresses(&self) -> json::JsonValue {
        json::JsonValue::Array(
            self.transparent_addresses
                .iter()
                .map(|(id, transparent_address)| {
                    json::object! {
                        "account" => u32::from(id.account_id()),
                        "address_index" => id.address_index().index(),
                        "scope" => id.scope().to_string(),
                        "encoded_address" => transparent_address.clone(),
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

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
                .unwrap_or(0),
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
            .unwrap_or(NonHardenedChildIndex::ZERO)
            .next()
            .ok_or(KeyError::InvalidNonHardenedChildIndex)?;
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
            .unwrap_or(NonHardenedChildIndex::ZERO)
            .next()
            .ok_or(KeyError::InvalidNonHardenedChildIndex)?
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
}
