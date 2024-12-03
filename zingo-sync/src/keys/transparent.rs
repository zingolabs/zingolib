//! Transparent keys and addresses

use zcash_address::{ToAddress as _, ZcashAddress};
use zcash_primitives::{
    consensus,
    legacy::{
        keys::{AccountPubKey, IncomingViewingKey as _, NonHardenedChildIndex},
        TransparentAddress,
    },
    zip32::AccountId,
};

use super::AddressIndex;

/// Unique ID for transparent addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TransparentAddressId {
    account_id: AccountId,
    scope: TransparentScope,
    address_index: AddressIndex,
}

impl TransparentAddressId {
    pub(crate) fn from_parts(
        account_id: zcash_primitives::zip32::AccountId,
        scope: TransparentScope,
        address_index: AddressIndex,
    ) -> Self {
        Self {
            account_id,
            scope,
            address_index,
        }
    }

    /// Gets address account ID
    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    /// Gets address scope
    pub fn scope(&self) -> TransparentScope {
        self.scope
    }

    /// Gets address index
    pub fn address_index(&self) -> AddressIndex {
        self.address_index
    }
}

/// Child index for the `change` path level in the BIP44 hierarchy (a.k.a. scope/chain).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransparentScope {
    /// External scope
    External,
    /// Internal scope (a.k.a. change)
    Internal,
    /// Refund scope (a.k.a. ephemeral)
    Refund,
}

pub(crate) fn derive_address<P>(
    consensus_parameters: &P,
    account_pubkey: &AccountPubKey,
    address_id: TransparentAddressId,
) -> String
where
    P: consensus::Parameters,
{
    let address = match address_id.scope() {
        TransparentScope::External => {
            derive_external_address(account_pubkey, address_id.address_index())
        }
        TransparentScope::Internal => {
            derive_internal_address(account_pubkey, address_id.address_index())
        }
        TransparentScope::Refund => {
            derive_refund_address(account_pubkey, address_id.address_index())
        }
    };

    encode_address(consensus_parameters, address)
}

fn derive_external_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> TransparentAddress {
    account_pubkey
        .derive_external_ivk()
        .unwrap()
        .derive_address(
            NonHardenedChildIndex::from_index(address_index)
                .expect("all non-hardened address indexes in use!"),
        )
        .unwrap()
}

fn derive_internal_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> TransparentAddress {
    account_pubkey
        .derive_internal_ivk()
        .unwrap()
        .derive_address(
            NonHardenedChildIndex::from_index(address_index)
                .expect("all non-hardened address indexes in use!"),
        )
        .unwrap()
}

fn derive_refund_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> TransparentAddress {
    account_pubkey
        .derive_ephemeral_ivk()
        .unwrap()
        .derive_ephemeral_address(
            NonHardenedChildIndex::from_index(address_index)
                .expect("all non-hardened address indexes in use!"),
        )
        .unwrap()
}

pub(crate) fn encode_address<P>(consensus_parameters: &P, address: TransparentAddress) -> String
where
    P: consensus::Parameters,
{
    let zcash_address = match address {
        TransparentAddress::PublicKeyHash(data) => {
            ZcashAddress::from_transparent_p2pkh(consensus_parameters.network_type(), data)
        }
        TransparentAddress::ScriptHash(data) => {
            ZcashAddress::from_transparent_p2sh(consensus_parameters.network_type(), data)
        }
    };
    zcash_address.to_string()
}
