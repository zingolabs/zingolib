//! This implements the Internal Capability

use std::{collections::{HashMap, HashSet}, sync::{atomic, Arc}};

use append_only_vec::AppendOnlyVec;
use zcash_keys::{address::UnifiedAddress, keys::DerivationError};
use zcash_primitives::{consensus::NetworkConstants, legacy::{keys::{AccountPubKey, IncomingViewingKey, NonHardenedChildIndex}, TransparentAddress}};
use zip32::DiversifierIndex;
use crate::{config::ChainType, wallet::error::KeyError};
use super::{legacy::generate_transparent_address_from_legacy_key, unified::{ReceiverSelection, UnifiedKeyStore, WalletCapability}, ToBase58Check};

pub (crate) trait InternalCapability: std::fmt::Debug + Send + Sync {
    fn get_ua_from_contained_transparent_receiver(
        &self,
        capability: &WalletCapability,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress>;

    fn addresses<'a>(&'a self, capability: &'a WalletCapability) -> &'a AppendOnlyVec<UnifiedAddress>;

    fn transparent_child_addresses<'a>(
        &'a self, 
        capability: &'a WalletCapability
    ) -> &'a Arc<AppendOnlyVec<(usize, TransparentAddress)>>;

    fn new_address(
        &self,
        capability: &WalletCapability,
        desired_receivers: ReceiverSelection,
        legacy_key: bool,
    ) -> Result<UnifiedAddress, String>;

    fn generate_transparent_receiver(
        &self,
        capability: &WalletCapability,
        // this should only be `true` when generating transparent addresses while loading from legacy keys (pre wallet version 29)
        // legacy transparent keys are already derived to the external scope so setting `legacy_key` to `true` will skip this scope derivation
        legacy_key: bool,
    ) -> Result<Option<TransparentAddress>, bip32::Error>;

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "not used in zingolib codebase")]
    fn get_taddr_to_secretkey_map(
        &self,
        capability: &WalletCapability,
        chain: &ChainType,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, KeyError>;

    fn get_external_taddrs(
        &self,
        capability: &WalletCapability,
        chain: &crate::config::ChainType
    ) -> HashSet<String>;

    fn get_taddrs(&self,
        capability: &WalletCapability,
        chain: &crate::config::ChainType
    ) -> HashSet<String>;

    fn first_sapling_address(&self, capability: &WalletCapability) -> sapling_crypto::PaymentAddress;

    fn get_trees_witness_trees(&self, capability: &WalletCapability) -> Option<crate::data::witness_trees::WitnessTrees>;

    fn can_view(&self, capability: &WalletCapability) -> ReceiverSelection;
}

#[derive(Debug)]
pub (crate) struct InMemoryWallet {}

impl InMemoryWallet {
    pub(crate) fn new() -> InMemoryWallet {
        InMemoryWallet{}
    }
}
impl InternalCapability for InMemoryWallet {
    fn get_ua_from_contained_transparent_receiver(
        &self,
        capability: &WalletCapability,  
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        capability.unified_addresses
            .iter()
            .find(|ua| ua.transparent() == Some(receiver))
            .cloned()
    }
    /// TODO: Add Doc Comment Here!
    fn addresses<'a>(&'a self, capability: &'a WalletCapability) -> &'a AppendOnlyVec<UnifiedAddress> {
        &capability.unified_addresses
    }

    /// TODO: Add Doc Comment Here!
    fn transparent_child_addresses<'a>(
        &'a self, 
        capability: &'a WalletCapability
    ) -> &'a Arc<AppendOnlyVec<(usize, TransparentAddress)>> {
        &capability.transparent_child_addresses
    }
    /// Generates a unified address from the given desired receivers
    ///
    /// See [`crate::wallet::WalletCapability::generate_transparent_receiver`] for information on using `legacy_key`
    fn new_address(
        &self,
        capability: &WalletCapability,
        desired_receivers: ReceiverSelection,
        legacy_key: bool,
    ) -> Result<UnifiedAddress, String> {
        if capability
            .addresses_write_lock
            .swap(true, atomic::Ordering::Acquire)
        {
            return Err("addresses_write_lock collision!".to_string());
        }

        let previous_num_addresses = capability.unified_addresses.len();
        let orchard_receiver = if desired_receivers.orchard {
            let fvk: orchard::keys::FullViewingKey = match capability.unified_key_store().try_into() {
                Ok(viewkey) => viewkey,
                Err(e) => {
                    capability.addresses_write_lock
                        .swap(false, atomic::Ordering::Release);
                    return Err(e.to_string());
                }
            };
            Some(fvk.address_at(capability.unified_addresses.len(), orchard::keys::Scope::External))
        } else {
            None
        };

        // produce a Sapling address to increment Sapling diversifier index
        let sapling_receiver = if desired_receivers.sapling {
            let mut sapling_diversifier_index = DiversifierIndex::new();
            let mut address;
            let mut count = 0;
            let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey =
                match capability.unified_key_store().try_into() {
                    Ok(viewkey) => viewkey,
                    Err(e) => {
                        capability.addresses_write_lock
                            .swap(false, atomic::Ordering::Release);
                        return Err(e.to_string());
                    }
                };
            loop {
                (sapling_diversifier_index, address) = fvk
                    .find_address(sapling_diversifier_index)
                    .expect("Diversifier index overflow");
                sapling_diversifier_index
                    .increment()
                    .expect("Diversifier index overflow");
                // Not all sapling_diversifier_indexes produce valid
                // sapling addresses.
                // Because of this self.unified_addresses.len()
                // will be <= sapling_diversifier_index
                if count == capability.unified_addresses.len() {
                    break;
                }
                count += 1;
            }
            Some(address)
        } else {
            None
        };

        let transparent_receiver = if desired_receivers.transparent {
            capability.generate_transparent_receiver(legacy_key)
                .map_err(|e| e.to_string())?
        } else {
            None
        };

        let ua = UnifiedAddress::from_receivers(
            orchard_receiver,
            sapling_receiver,
            transparent_receiver,
        );
        let ua = match ua {
            Some(address) => address,
            None => {
                capability.addresses_write_lock
                    .swap(false, atomic::Ordering::Release);
                return Err(
                    "Invalid receivers requested! At least one of sapling or orchard required"
                        .to_string(),
                );
            }
        };
        capability.unified_addresses.push(ua.clone());
        assert_eq!(capability.unified_addresses.len(), previous_num_addresses + 1);
        capability.addresses_write_lock
            .swap(false, atomic::Ordering::Release);
        Ok(ua)
    }

    /// Generates a transparent receiver for the specified scope.
    fn generate_transparent_receiver(
        &self,
        capability: &WalletCapability,
        // this should only be `true` when generating transparent addresses while loading from legacy keys (pre wallet version 29)
        // legacy transparent keys are already derived to the external scope so setting `legacy_key` to `true` will skip this scope derivation
        legacy_key: bool,
    ) -> Result<Option<TransparentAddress>, bip32::Error> {
        let derive_address = |transparent_fvk: &AccountPubKey,
                              child_index: NonHardenedChildIndex|
         -> Result<TransparentAddress, bip32::Error> {
            let t_addr = if legacy_key {
                generate_transparent_address_from_legacy_key(transparent_fvk, child_index)?
            } else {
                transparent_fvk
                    .derive_external_ivk()?
                    .derive_address(child_index)?
            };

            capability.transparent_child_addresses
                .push((capability.addresses().len(), t_addr));
            Ok(t_addr)
        };
        let child_index = NonHardenedChildIndex::from_index(capability.addresses().len() as u32)
            .expect("hardened bit should not be set for non-hardened child indexes");
        let transparent_receiver = match capability.unified_key_store() {
            UnifiedKeyStore::Spend(usk) => {
                derive_address(&usk.transparent().to_account_pubkey(), child_index)
                    .map(Option::Some)
            }
            UnifiedKeyStore::View(ufvk) => ufvk
                .transparent()
                .map(|pub_key| derive_address(pub_key, child_index))
                .transpose(),
            UnifiedKeyStore::Empty => Ok(None),
        }?;

        Ok(transparent_receiver)
    }

    /// TODO: Add Doc Comment Here!
    fn get_taddr_to_secretkey_map(
        &self,
        capability: &WalletCapability,
        chain: &ChainType,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, KeyError> {
        if let UnifiedKeyStore::Spend(usk) = capability.unified_key_store() {
            capability.transparent_child_addresses()
                .iter()
                .map(|(i, taddr)| -> Result<_, KeyError> {
                    let hash = match taddr {
                        TransparentAddress::PublicKeyHash(hash) => hash,
                        TransparentAddress::ScriptHash(hash) => hash,
                    };
                    Ok((
                        hash.to_base58check(&chain.b58_script_address_prefix(), &[]),
                        usk.transparent()
                            .derive_external_secret_key(
                                NonHardenedChildIndex::from_index(*i as u32)
                                    .ok_or(KeyError::InvalidNonHardenedChildIndex)?,
                            )
                            .map_err(DerivationError::Transparent)
                            .map_err(KeyError::KeyDerivationError)?,
                    ))
                })
                .collect::<Result<_, _>>()
        } else {
            Err(KeyError::NoSpendCapability)
        }
    }

    /// external here refers to HD keys:
    /// <https://zips.z.cash/zip-0032>
    /// where external and internal were inherited from the BIP44 conventions
    fn get_external_taddrs(
        &self,
        capability: &WalletCapability,
        chain: &crate::config::ChainType) -> HashSet<String> {
        capability.unified_addresses
            .iter()
            .filter_map(|address| {
                address.transparent().and_then(|transparent_receiver| {
                    if let zcash_primitives::legacy::TransparentAddress::PublicKeyHash(hash) =
                        transparent_receiver
                    {
                        Some(super::ToBase58Check::to_base58check(
                            hash.as_slice(),
                            &chain.b58_pubkey_address_prefix(),
                            &[],
                        ))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    /// TODO: This does not appear to be used
    fn get_taddrs(&self,
        capability: &WalletCapability,
        chain: &crate::config::ChainType
    ) -> HashSet<String> {
        self.get_external_taddrs(capability, chain,)
            .union(&capability.get_rejection_address_set(chain))
            .cloned()
            .collect()
    }
    /// TODO: Add Doc Comment Here!
    fn first_sapling_address(
        &self,
        capability: &WalletCapability
    ) -> sapling_crypto::PaymentAddress {
        // This index is dangerous, but all ways to instantiate a UnifiedSpendAuthority
        // create it with a suitable first address
        *capability.addresses()[0].sapling().unwrap()
    }

    /// TODO: Add Doc Comment Here!
    //TODO: NAME?????!!
    fn get_trees_witness_trees(
        &self,
        capability: &WalletCapability,
    ) -> Option<crate::data::witness_trees::WitnessTrees> {
        if capability.unified_key_store().is_spending_key() {
            Some(crate::data::witness_trees::WitnessTrees::default())
        } else {
            None
        }
    }

    /// Returns a selection of pools where the wallet can view funds.
    fn can_view(
        &self,
        capability: &WalletCapability,
    ) -> ReceiverSelection {
        match capability.unified_key_store() {
            UnifiedKeyStore::Spend(_) => ReceiverSelection {
                orchard: true,
                sapling: true,
                transparent: true,
            },
            UnifiedKeyStore::View(ufvk) => ReceiverSelection {
                orchard: ufvk.orchard().is_some(),
                sapling: ufvk.sapling().is_some(),
                transparent: ufvk.transparent().is_some(),
            },
            UnifiedKeyStore::Empty => ReceiverSelection {
                orchard: false,
                sapling: false,
                transparent: false,
            },
        }
    }
}