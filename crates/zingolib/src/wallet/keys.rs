//! [`crate::wallet::LightWallet`] methods associated with keys and address derivation.

use std::collections::BTreeSet;

use pepper_sync::{
    keys::{
        decode_address,
        transparent::{self, TransparentAddressId, TransparentScope},
    },
    wallet::{KeyIdInterface, TransparentCoin},
};
use unified::{ReceiverSelection, UnifiedAddressId, UnifiedKeyStore};
use zcash_keys::address::{Address, UnifiedAddress};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::keys::NonHardenedChildIndex;
use zip32::DiversifierIndex;

use super::{LightWallet, error::KeyError};

pub mod unified;

/// The transparent address a wallet stores encoded, decoded for `network`.
///
/// The wallet keeps its transparent addresses as strings. Panics when the
/// string is not a transparent address of `network`, which the wallet load
/// has already checked.
pub fn transparent_address_from_encoded(
    network: zcash_protocol::consensus::NetworkType,
    encoded: &str,
) -> Result<TransparentAddress, zcash_address::ParseError> {
    let address = zcash_address::ZcashAddress::try_from_encoded(encoded)?
        .convert_if_network::<Address>(network)
        .expect("incorrect network should be checked on wallet load");
    match address {
        Address::Transparent(address) => Ok(address),
        _ => panic!("a stored transparent address decodes to a transparent address"),
    }
}

pub enum WalletAddressRef {
    Unified {
        account_id: zip32::AccountId,
        address_index: Option<u32>,
        has_orchard: bool,
        has_sapling: bool,
        has_transparent: bool,
        encoded_address: String,
    },
    OrchardInternal {
        account_id: zip32::AccountId,
        diversifier_index: DiversifierIndex,
        encoded_address: String,
    },
    SaplingExternal {
        account_id: zip32::AccountId,
        diversifier_index: DiversifierIndex,
        encoded_address: String,
    },
    Transparent {
        account_id: zip32::AccountId,
        scope: TransparentScope,
        address_index: NonHardenedChildIndex,
        encoded_address: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletKind {
    Mnemonic,
    SpendingKey,
    ViewingKey {
        transparent: bool,
        sapling: bool,
        orchard: bool,
    },
    NoKeys,
}

impl LightWallet {
    pub fn kind(&self) -> Result<WalletKind, KeyError> {
        if self.mnemonic_phrase().is_some() {
            return Ok(WalletKind::Mnemonic);
        }
        let kind = match self
            .unified_key_store
            .get(&zip32::AccountId::ZERO)
            .ok_or(KeyError::NoAccountKeys)?
        {
            UnifiedKeyStore::Spend(_) => WalletKind::SpendingKey,
            UnifiedKeyStore::View(ufvk) => WalletKind::ViewingKey {
                transparent: ufvk.p2pkh().is_some(),
                sapling: ufvk.sapling().is_some(),
                orchard: ufvk.orchard().is_some(),
            },
            UnifiedKeyStore::Empty => WalletKind::NoKeys,
        };
        Ok(kind)
    }

    /// Returns a new unified address for the given `receivers` and `account_id`, adding this new unified address to
    /// the wallet.
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
            .generate_unified_address(address_id.address_index, receivers)?;
        self.unified_addresses
            .insert(address_id, unified_address.clone());
        self.save_required = true;

        Ok((address_id, unified_address))
    }

    /// Generates a new transparent address of `external` scope for the given `account_id`.
    /// The new address is added to the wallet and returned.
    ///
    /// If `enforced_no_gap` is `true`, an error is returned if the latest transparent address has not received funds.
    pub fn generate_transparent_address(
        &mut self,
        account_id: zip32::AccountId,
        enforce_no_gap: bool,
    ) -> Result<(TransparentAddressId, TransparentAddress), KeyError> {
        let latest_address = self
            .transparent_addresses
            .iter()
            .filter(|(address_id, _)| {
                address_id.scope() == TransparentScope::External
                    && address_id.account_id() == account_id
            })
            .max_by_key(|(address_id, _)| address_id.address_index());
        if enforce_no_gap
            && let Some((_, address)) = latest_address
            && !self
                .wallet_outputs::<TransparentCoin>()
                .iter()
                .any(|&output| output.address() == address.as_str())
        {
            return Err(KeyError::GapError);
        }

        let address_index =
            latest_address.map_or(Ok(NonHardenedChildIndex::ZERO), |(address_index, _)| {
                address_index
                    .address_index()
                    .next()
                    .ok_or(KeyError::InvalidNonHardenedChildIndex)
            })?;
        let address_id =
            TransparentAddressId::new(account_id, TransparentScope::External, address_index);
        let external_address = self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .generate_transparent_address(address_id.address_index(), address_id.scope())?;
        self.transparent_addresses.insert(
            address_id,
            transparent::encode_address(&self.chain_type, external_address),
        );
        self.save_required = true;

        Ok((address_id, external_address))
    }

    /// Derives, without reserving, the next `n` refund-scope (ephemeral)
    /// transparent addresses for the given `account_id`. Pure: the wallet
    /// is not modified, so an abandoned proposal leaves no trace and its
    /// indexes are reused. Reservation — insertion into the wallet's
    /// address book — happens only when a transaction bearing the address
    /// comes into existence (ADR 0010).
    ///
    /// Public so a caller can name the address a coming proposal will spend
    /// through to a counterparty that wants it before the transaction exists.
    /// A swap provider that reads the refund destination off the deposit's
    /// origin is the case this serves. Every call answers with the same
    /// indexes until an apply reserves them, so a quote the user walks away
    /// from leaves the gap limit where it was. A proposal applied in between
    /// claims those indexes, and the next call answers with the ones after.
    pub fn derive_refund_addresses(
        &self,
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

        (first_index..(first_index + n))
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
                    .generate_transparent_address(address_id.address_index(), address_id.scope())?;

                Ok((address_id, refund_address))
            })
            .collect()
    }

    /// Generates 'n' new transparent addresses of `refund` (ephemeral) scope for the given `account_id`.
    /// The new addresses are added to the wallet and returned.
    pub fn generate_refund_addresses(
        &mut self,
        n: usize,
        account_id: zip32::AccountId,
    ) -> Result<Vec<(TransparentAddressId, TransparentAddress)>, KeyError> {
        let refund_addresses = self.derive_refund_addresses(n, account_id)?;
        for (address_id, refund_address) in &refund_addresses {
            self.transparent_addresses.insert(
                *address_id,
                transparent::encode_address(&self.chain_type, *refund_address),
            );
        }
        self.save_required = true;

        Ok(refund_addresses)
    }

    /// Returns a [`crate::wallet::keys::WalletAddressRef`] if the `encoded_address` is in the wallet's address lists.
    ///
    /// Does not detect internal sapling and orchard addresses.
    pub fn is_wallet_address(
        &self,
        encoded_address: &str,
    ) -> Result<Option<WalletAddressRef>, KeyError> {
        Ok(match decode_address(&self.chain_type, encoded_address)? {
            zcash_keys::address::Address::Unified(address) => {
                let orchard = address
                    .orchard()
                    .and_then(|address| self.is_orchard_address_in_unified_addresses(address));
                let sapling = address
                    .sapling()
                    .and_then(|address| self.is_sapling_address_in_unified_addresses(address));
                let transparent = address
                    .transparent()
                    .and_then(|address| self.is_transparent_wallet_address(address))
                    .filter(|address_id| address_id.scope() == TransparentScope::External);

                if let Some((unified_address_id, _unified_address)) = orchard {
                    // a unified address index will not be assigned if the orchard and sapling receivers have different
                    // unified address ids
                    let address_index = sapling.as_ref().map_or(
                        Some(unified_address_id.address_index),
                        |(id, _address)| {
                            if *id == unified_address_id {
                                Some(unified_address_id.address_index)
                            } else {
                                None
                            }
                        },
                    );
                    Some(WalletAddressRef::Unified {
                        account_id: unified_address_id.account_id,
                        address_index,
                        has_orchard: true,
                        has_sapling: sapling.is_some(),
                        has_transparent: transparent.is_some(),
                        encoded_address: encoded_address.to_string(),
                    })
                } else if let Some((unified_address_id, _unified_address)) = sapling {
                    Some(WalletAddressRef::Unified {
                        account_id: unified_address_id.account_id,
                        address_index: Some(unified_address_id.address_index),
                        has_orchard: false,
                        has_sapling: true,
                        has_transparent: transparent.is_some(),
                        encoded_address: encoded_address.to_string(),
                    })
                } else {
                    None
                }
            }
            zcash_keys::address::Address::Sapling(address) => {
                self.is_sapling_address_in_unified_addresses(&address).map(
                    |(unified_address_id, unified_address)| WalletAddressRef::Unified {
                        account_id: unified_address_id.account_id,
                        address_index: Some(unified_address_id.address_index),
                        has_orchard: unified_address.has_orchard(),
                        has_sapling: true,
                        has_transparent: unified_address.has_transparent(),
                        encoded_address: encoded_address.to_string(),
                    },
                )
            }
            zcash_keys::address::Address::Transparent(address) => self
                .is_transparent_wallet_address(&address)
                .map(|address_id| WalletAddressRef::Transparent {
                    account_id: address_id.account_id(),
                    scope: address_id.scope(),
                    address_index: address_id.address_index(),
                    encoded_address: encoded_address.to_string(),
                }),
            zcash_keys::address::Address::Tex(_) => None,
        })
    }

    /// Returns a [`crate::wallet::keys::WalletAddressRef`] if the `encoded_address` was derived by the wallet's keys.
    ///
    /// This method is computationally expensive.
    ///
    /// Fails to detect internal sapling addresses.
    /// <https://github.com/zcash/sapling-crypto/issues/160>
    pub fn is_address_derived_by_keys(
        &self,
        encoded_address: &str,
    ) -> Result<Option<WalletAddressRef>, KeyError> {
        Ok(match decode_address(&self.chain_type, encoded_address)? {
            zcash_keys::address::Address::Unified(address) => {
                let orchard = address
                    .orchard()
                    .and_then(|address| self.is_orchard_address_derived_from_fvks(address));
                let sapling = address
                    .sapling()
                    .and_then(|address| self.is_sapling_address_derived_from_fvks(address));
                let transparent = address
                    .transparent()
                    .and_then(|address| self.is_transparent_wallet_address(address))
                    .filter(|address_id| address_id.scope() == TransparentScope::External);

                if let Some((account_id, scope, orchard_diversifier_index)) = orchard {
                    if scope == zip32::Scope::External {
                        // a unified address index will not be assigned if it does not match the address in the wallet
                        let address_index = u32::try_from(orchard_diversifier_index).ok().and_then(
                            |address_index| {
                                self.unified_addresses()
                                    .get(&UnifiedAddressId {
                                        account_id,
                                        address_index,
                                    })
                                    .and_then(|unified_address| {
                                        if *unified_address == *address {
                                            Some(address_index)
                                        } else {
                                            None
                                        }
                                    })
                            },
                        );
                        Some(WalletAddressRef::Unified {
                            account_id,
                            address_index,
                            has_orchard: true,
                            has_sapling: sapling.is_some(),
                            has_transparent: transparent.is_some(),
                            encoded_address: encoded_address.to_string(),
                        })
                    } else if scope == zip32::Scope::Internal {
                        Some(WalletAddressRef::OrchardInternal {
                            account_id,
                            diversifier_index: orchard_diversifier_index,
                            encoded_address: encoded_address.to_string(),
                        })
                    } else {
                        unreachable!("Only external and internal scopes exist!");
                    }
                } else if let Some((account_id, diversifier_index)) = sapling {
                    // a unified address index will not be assigned if it does not match the address in the wallet
                    let address_index = Some(
                        self.unified_key_store
                            .get(&account_id)
                            .expect("key must exist in this scope")
                            .determine_nth_valid_sapling_diversifier(diversifier_index)
                            .expect("key must exist in this scope")
                            - 1,
                    )
                    .and_then(|address_index| {
                        self.unified_addresses()
                            .get(&UnifiedAddressId {
                                account_id,
                                address_index,
                            })
                            .and_then(|unified_address| {
                                if *unified_address == *address {
                                    Some(address_index)
                                } else {
                                    None
                                }
                            })
                    });

                    Some(WalletAddressRef::Unified {
                        account_id,
                        address_index,
                        has_orchard: false,
                        has_sapling: true,
                        has_transparent: transparent.is_some(),
                        encoded_address: encoded_address.to_string(),
                    })
                } else {
                    None
                }
            }
            zcash_keys::address::Address::Sapling(address) => {
                self.is_sapling_address_derived_from_fvks(&address).map(
                    |(account_id, diversifier_index)| WalletAddressRef::SaplingExternal {
                        account_id,
                        diversifier_index,
                        encoded_address: encoded_address.to_string(),
                    },
                )
            }
            zcash_keys::address::Address::Transparent(address) => self
                .is_transparent_wallet_address(&address)
                .map(|address_id| WalletAddressRef::Transparent {
                    account_id: address_id.account_id(),
                    scope: address_id.scope(),
                    address_index: address_id.address_index(),
                    encoded_address: encoded_address.to_string(),
                }),
            zcash_keys::address::Address::Tex(_) => None,
        })
    }

    /// Returns the address identifier if the given `address` is one of the wallet's derived addresses.
    #[must_use]
    pub fn is_transparent_wallet_address(
        &self,
        address: &TransparentAddress,
    ) -> Option<TransparentAddressId> {
        let encoded_address = transparent::encode_address(&self.chain_type, *address);

        self.transparent_addresses
            .iter()
            .find(|(_, wallet_address)| **wallet_address == encoded_address)
            .map(|(address_id, _)| *address_id)
    }

    /// Returns the account id and diversifier index if the given `address` is derived from the wallet's sapling FVKs. External scope only.
    ///
    /// This method is computationally expensive.
    #[must_use]
    pub fn is_sapling_address_derived_from_fvks(
        &self,
        address: &sapling_crypto::PaymentAddress,
    ) -> Option<(zip32::AccountId, DiversifierIndex)> {
        for (account_id, unified_key) in &self.unified_key_store {
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

    /// Returns the account id, scope and diversifier index if the given `address` is derived from the wallet's orchard FVKs.
    ///
    /// This method is computationally expensive.
    #[must_use]
    pub fn is_orchard_address_derived_from_fvks(
        &self,
        address: &orchard::Address,
    ) -> Option<(zip32::AccountId, zip32::Scope, DiversifierIndex)> {
        for (account_id, unified_key) in &self.unified_key_store {
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

    /// Returns the unified address and id if `address` matches an sapling receiver in the wallet's unified address list.
    #[must_use]
    pub fn is_sapling_address_in_unified_addresses(
        &self,
        address: &sapling_crypto::PaymentAddress,
    ) -> Option<(UnifiedAddressId, UnifiedAddress)> {
        self.unified_addresses
            .iter()
            .find(|(_, unified_address)| unified_address.sapling() == Some(address))
            .map(|(id, address)| (*id, address.clone()))
    }

    /// Returns the unified address and id if `address` matches an orchard receiver in the wallet's unified address list.
    #[must_use]
    pub fn is_orchard_address_in_unified_addresses(
        &self,
        address: &orchard::Address,
    ) -> Option<(UnifiedAddressId, UnifiedAddress)> {
        self.unified_addresses
            .iter()
            .find(|(_, unified_address)| unified_address.orchard() == Some(address))
            .map(|(id, address)| (*id, address.clone()))
    }

    pub(crate) fn highest_refund_address_index(&self) -> Option<NonHardenedChildIndex> {
        self.transparent_addresses()
            .keys()
            .filter(|id| id.scope() == TransparentScope::Refund)
            .max_by_key(|id| id.address_index())
            .map(|id| id.address_index())
    }

    pub(crate) fn truncate_failed_refund_addresses(&mut self) {
        let accounts: BTreeSet<zip32::AccountId> = self
            .transparent_addresses()
            .keys()
            .filter(|id| id.scope() == TransparentScope::Refund)
            .map(|id| id.account_id())
            .collect();
        for account_id in accounts {
            self.truncate_failed_refund_addresses_for_account(account_id);
        }
    }

    fn truncate_failed_refund_addresses_for_account(&mut self, account_id: zip32::AccountId) {
        loop {
            let Some((id, address)) = self
                .transparent_addresses()
                .iter()
                .filter(|(id, _)| {
                    id.scope() == TransparentScope::Refund && id.account_id() == account_id
                })
                .max_by_key(|(id, _)| id.address_index())
                .map(|(id, address)| (*id, address.clone()))
            else {
                return;
            };
            let mut paid = false;
            let mut live = false;
            for transaction in self.wallet_transactions.values() {
                if transaction
                    .transparent_coins()
                    .iter()
                    .any(|coin| coin.address() == address)
                {
                    paid = true;
                    live |= !transaction.status().is_failed();
                }
            }
            if !paid || live {
                return;
            }
            self.transparent_addresses_mut().remove(&id);
        }
    }

    /// Removes any refund address in the wallet above the given index.
    ///
    /// If `index_opt` is `None`, remove all refund addresses.
    pub(crate) fn truncate_refund_addresses(&mut self, index_opt: Option<NonHardenedChildIndex>) {
        if let Some(current_highest_index) = self.highest_refund_address_index()
            && index_opt.is_none_or(|index| current_highest_index > index)
        {
            self.transparent_addresses_mut().retain(|id, _| {
                if let Some(index) = index_opt {
                    !(id.scope() == TransparentScope::Refund && id.address_index() > index)
                } else {
                    id.scope() != TransparentScope::Refund
                }
            });
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
mod test {
    use zcash_protocol::PoolType;

    use crate::wallet::LightWallet;

    use super::unified::UnifiedAddressId;

    impl LightWallet {
        /// Returns an encoded address for a given `pool`.
        ///
        /// Zingolib test framework generates a second UA with a sapling only receiver for use when `pool` is set to sapling.
        // TODO: add asserts to verify UA receivers
        #[must_use]
        pub fn get_address(&self, pool: PoolType) -> String {
            match pool {
                // The ironwood receiver of a unified address is its orchard
                // receiver.
                PoolType::IRONWOOD | PoolType::ORCHARD => self
                    .unified_addresses()
                    .get(&UnifiedAddressId {
                        address_index: 0,
                        account_id: zip32::AccountId::ZERO,
                    })
                    .unwrap()
                    .encode(&self.chain_type),
                PoolType::SAPLING => self
                    .unified_addresses()
                    .get(&UnifiedAddressId {
                        address_index: 1,
                        account_id: zip32::AccountId::ZERO,
                    })
                    .unwrap()
                    .encode(&self.chain_type),
                PoolType::Transparent => {
                    self.transparent_addresses.values().next().unwrap().clone()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
