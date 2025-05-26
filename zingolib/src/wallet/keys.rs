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
                        "has_orchard" => unified_address.has_sapling(),
                        "has_sapling" => unified_address.has_orchard(),
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
