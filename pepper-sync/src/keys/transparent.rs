//! Transparent keys and addresses

use zcash_address::{ToAddress as _, ZcashAddress};
use zcash_primitives::{
    consensus,
    legacy::{
        keys::{
            AccountPubKey, IncomingViewingKey as _, NonHardenedChildIndex, TransparentKeyScope,
        },
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
    /// Construct from parts
    pub fn new(
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

impl std::fmt::Display for TransparentScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TransparentScope::External => "external",
                TransparentScope::Internal => "internal",
                TransparentScope::Refund => "refund",
            }
        )
    }
}

impl From<TransparentScope> for TransparentKeyScope {
    fn from(value: TransparentScope) -> Self {
        match value {
            TransparentScope::External => TransparentKeyScope::EXTERNAL,
            TransparentScope::Internal => TransparentKeyScope::INTERNAL,
            TransparentScope::Refund => TransparentKeyScope::EPHEMERAL,
        }
    }
}

impl TryFrom<u8> for TransparentScope {
    type Error = std::io::Error;

    fn try_from(value: u8) -> std::io::Result<Self> {
        match value {
            0 => Ok(TransparentScope::External),
            1 => Ok(TransparentScope::Internal),
            2 => Ok(TransparentScope::Refund),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid scope value",
            )),
        }
    }
}

pub(crate) fn derive_address(
    consensus_parameters: &impl consensus::Parameters,
    account_pubkey: &AccountPubKey,
    address_id: TransparentAddressId,
) -> Result<String, bip32::Error> {
    let address = match address_id.scope() {
        TransparentScope::External => {
            derive_external_address(account_pubkey, address_id.address_index())?
        }
        TransparentScope::Internal => {
            derive_internal_address(account_pubkey, address_id.address_index())?
        }
        TransparentScope::Refund => {
            derive_refund_address(account_pubkey, address_id.address_index())?
        }
    };

    Ok(encode_address(consensus_parameters, address))
}

fn derive_external_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey.derive_external_ivk()?.derive_address(
        NonHardenedChildIndex::from_index(address_index)
            .expect("all non-hardened address indexes in use!"),
    )
}

fn derive_internal_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey.derive_internal_ivk()?.derive_address(
        NonHardenedChildIndex::from_index(address_index)
            .expect("all non-hardened address indexes in use!"),
    )
}

fn derive_refund_address(
    account_pubkey: &AccountPubKey,
    address_index: AddressIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey
        .derive_ephemeral_ivk()?
        .derive_ephemeral_address(
            NonHardenedChildIndex::from_index(address_index)
                .expect("all non-hardened address indexes in use!"),
        )
}

/// Encodes transparent address
pub fn encode_address(
    consensus_parameters: &impl consensus::Parameters,
    address: TransparentAddress,
) -> String {
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
