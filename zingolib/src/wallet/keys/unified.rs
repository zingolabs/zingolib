//! TODO: Add Mod Description Here!

use std::io::{self, Read, Write};

use bip0039::Mnemonic;
use byteorder::{ReadBytesExt, WriteBytesExt};

use pepper_sync::keys::transparent::TransparentScope;
use zcash_address::unified::{Encoding as _, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_encoding::CompactSize;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{NetworkConstants, Parameters};
use zcash_primitives::legacy::{
    TransparentAddress,
    keys::{IncomingViewingKey, NonHardenedChildIndex},
};
use zcash_primitives::zip32::{AccountId, DiversifierIndex};

use crate::config::ChainType;
use crate::wallet::error::KeyError;
use crate::wallet::traits::ReadableWriteable;

use super::legacy::generate_transparent_address_from_legacy_key;

pub(crate) const KEY_TYPE_EMPTY: u8 = 0;
pub(crate) const KEY_TYPE_VIEW: u8 = 1;
pub(crate) const KEY_TYPE_SPEND: u8 = 2;

/// Unique ID for unified addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnifiedAddressId {
    pub account_id: AccountId,
    pub address_index: u32,
}

/// In-memory store for wallet spending or viewing keys
#[derive(Debug)]
pub enum UnifiedKeyStore {
    /// Wallet with spend capability
    Spend(Box<UnifiedSpendingKey>),
    /// Wallet with view capability
    View(Box<UnifiedFullViewingKey>),
    /// Wallet with no keys
    Empty,
}

impl UnifiedKeyStore {
    /// Create a unified key store from raw entropy (64-byte seed).
    pub fn new_from_seed(
        network: &ChainType,
        seed: &[u8; 64],
        account_index: zip32::AccountId,
    ) -> Result<Self, KeyError> {
        let usk = UnifiedSpendingKey::from_seed(network, seed, account_index)
            .map_err(KeyError::KeyDerivationError)?;

        Ok(UnifiedKeyStore::Spend(Box::new(usk)))
    }

    /// Create a unified key store from a mnemonic.
    ///
    /// Refer to BIP-0039 for details on seed generation from mnemonic phrases.
    pub fn new_from_mnemonic(
        network: &ChainType,
        mnemonic: &Mnemonic,
        account_index: zip32::AccountId,
    ) -> Result<Self, KeyError> {
        let seed = mnemonic.to_seed("");
        Self::new_from_seed(network, &seed, account_index)
    }

    /// Create a unified key store from unified spending key bytes.
    pub fn new_from_usk(usk: &[u8]) -> Result<Self, KeyError> {
        let usk = UnifiedSpendingKey::from_bytes(Era::Orchard, usk)
            .map_err(|_| KeyError::KeyDecodingError)?;

        Ok(UnifiedKeyStore::Spend(Box::new(usk)))
    }

    /// Create a unified key store from unified full viewing key encoded string.
    pub fn new_from_ufvk(network: &ChainType, ufvk_encoded: String) -> Result<Self, KeyError> {
        if ufvk_encoded.starts_with(network.hrp_sapling_extended_full_viewing_key()) {
            return Err(KeyError::InvalidFormat);
        }
        let (network_type, ufvk) =
            Ufvk::decode(&ufvk_encoded).map_err(|_| KeyError::KeyDecodingError)?;
        if network_type != network.network_type() {
            return Err(KeyError::NetworkMismatch);
        }
        let ufvk = UnifiedFullViewingKey::parse(&ufvk).map_err(|_| KeyError::KeyDecodingError)?;

        Ok(UnifiedKeyStore::View(Box::new(ufvk)))
    }

    /// Returns true if [`UnifiedKeyStore`] is of `Spend` variant
    pub fn is_spending_key(&self) -> bool {
        matches!(self, UnifiedKeyStore::Spend(_))
    }

    /// Returns true if [`UnifiedKeyStore`] is of `Empty` variant
    pub fn is_empty(&self) -> bool {
        matches!(self, UnifiedKeyStore::Empty)
    }

    /// Returns a selection of pools where the wallet can view funds.
    pub fn can_view(&self) -> ReceiverSelection {
        match self {
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

    /// Generates a unified address for the given `unified_address_index` and `receivers`.
    ///
    /// See [`self::UnifiedKeyStore::generate_transparent_address`] for information on using `legacy_key`.
    pub fn generate_unified_address(
        &self,
        unified_address_index: u32,
        receivers: ReceiverSelection,
        legacy_key: bool,
    ) -> Result<UnifiedAddress, KeyError> {
        let orchard_receiver = if receivers.orchard {
            let fvk = orchard::keys::FullViewingKey::try_from(self)?;
            Some(fvk.address_at(unified_address_index, orchard::keys::Scope::External))
        } else {
            None
        };

        let sapling_receiver = if receivers.sapling {
            let mut sapling_diversifier_index = DiversifierIndex::new();
            let mut address;
            let mut count = 0;
            let fvk = sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(self)?;

            // not all sapling_diversifier_indexes produce valid sapling addresses.
            // therefore, `sapling_diversifier_index` may be larger than `ua_index` and only the valid payment
            // addresses are counted.
            loop {
                (sapling_diversifier_index, address) = fvk
                    .find_address(sapling_diversifier_index)
                    .expect("Diversifier index overflow");
                sapling_diversifier_index
                    .increment()
                    .expect("Diversifier index overflow");
                if count == unified_address_index {
                    break;
                }
                count += 1;
            }
            Some(address)
        } else {
            None
        };

        let transparent_receiver = if receivers.transparent {
            Some(self.generate_transparent_address(
                unified_address_index,
                TransparentScope::External,
                legacy_key,
            )?)
        } else {
            None
        };

        let unified_address = UnifiedAddress::from_receivers(
            orchard_receiver,
            sapling_receiver,
            transparent_receiver,
        )
        .ok_or(KeyError::UnifiedAddressError)?;

        Ok(unified_address)
    }

    /// Generates a transparent address for the given `address_index` and `scope`.
    ///
    /// Panics if `address_index` has the hardened bit set.
    pub fn generate_transparent_address(
        &self,
        address_index: u32,
        scope: TransparentScope,
        // this should only be `true` when generating externally scoped transparent addresses while loading from legacy
        // keys (pre wallet version 29).
        // legacy transparent keys are already derived to the external scope so setting `legacy_key` to `true` will
        // skip this scope derivation.
        legacy_key: bool,
    ) -> Result<TransparentAddress, KeyError> {
        let child_index = NonHardenedChildIndex::from_index(address_index)
            .expect("hardened bit should not be set for non-hardened child indexes");
        let account_pubkey = UnifiedFullViewingKey::try_from(self)?
            .transparent()
            .ok_or(KeyError::NoViewCapability)?
            .clone();

        let transparent_address = match scope {
            TransparentScope::External => {
                if legacy_key {
                    generate_transparent_address_from_legacy_key(&account_pubkey, child_index)?
                } else {
                    account_pubkey
                        .derive_external_ivk()?
                        .derive_address(child_index)?
                }
            }
            TransparentScope::Internal => account_pubkey
                .derive_internal_ivk()?
                .derive_address(child_index)?,
            TransparentScope::Refund => account_pubkey
                .derive_ephemeral_ivk()?
                .derive_ephemeral_address(child_index)?,
        };

        Ok(transparent_address)
    }
}

impl ReadableWriteable<ChainType, ChainType> for UnifiedKeyStore {
    const VERSION: u8 = 0;
    fn read<R: Read>(mut reader: R, input: ChainType) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let key_type = reader.read_u8()?;
        Ok(match key_type {
            KEY_TYPE_SPEND => {
                UnifiedKeyStore::Spend(Box::new(UnifiedSpendingKey::read(reader, ())?))
            }
            KEY_TYPE_VIEW => {
                UnifiedKeyStore::View(Box::new(UnifiedFullViewingKey::read(reader, input)?))
            }
            KEY_TYPE_EMPTY => UnifiedKeyStore::Empty,
            x => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unknown key type: {}", x),
                ));
            }
        })
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        match self {
            UnifiedKeyStore::Spend(usk) => {
                writer.write_u8(KEY_TYPE_SPEND)?;
                usk.write(&mut writer, ())
            }
            UnifiedKeyStore::View(ufvk) => {
                writer.write_u8(KEY_TYPE_VIEW)?;
                ufvk.write(&mut writer, input)
            }
            UnifiedKeyStore::Empty => writer.write_u8(KEY_TYPE_EMPTY),
        }
    }
}
impl ReadableWriteable for UnifiedSpendingKey {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let len = CompactSize::read(&mut reader)?;
        let mut usk = vec![0u8; len as usize];
        reader.read_exact(&mut usk)?;

        UnifiedSpendingKey::from_bytes(Era::Orchard, &usk)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "USK bytes are invalid"))
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        let usk_bytes = self.to_bytes(Era::Orchard);
        CompactSize::write(&mut writer, usk_bytes.len())?;
        writer.write_all(&usk_bytes)?;
        Ok(())
    }
}
impl ReadableWriteable<ChainType, ChainType> for UnifiedFullViewingKey {
    const VERSION: u8 = 0;

    fn read<R: Read>(mut reader: R, input: ChainType) -> io::Result<Self> {
        let len = CompactSize::read(&mut reader)?;
        let mut ufvk = vec![0u8; len as usize];
        reader.read_exact(&mut ufvk)?;
        let ufvk_encoded = std::str::from_utf8(&ufvk)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        UnifiedFullViewingKey::decode(&input, ufvk_encoded).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("UFVK decoding error: {}", e),
            )
        })
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        let ufvk_bytes = self.encode(&input).as_bytes().to_vec();
        CompactSize::write(&mut writer, ufvk_bytes.len())?;
        writer.write_all(&ufvk_bytes)?;
        Ok(())
    }
}

impl TryFrom<&UnifiedKeyStore> for UnifiedSpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        match unified_key_store {
            UnifiedKeyStore::Spend(usk) => Ok(*usk.clone()),
            _ => Err(KeyError::NoSpendCapability),
        }
    }
}
impl TryFrom<&UnifiedKeyStore> for orchard::keys::SpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(*usk.orchard())
    }
}
impl TryFrom<&UnifiedKeyStore> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(usk.sapling().clone())
    }
}
impl TryFrom<&UnifiedKeyStore> for zcash_primitives::legacy::keys::AccountPrivKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let usk = UnifiedSpendingKey::try_from(unified_key_store)?;
        Ok(usk.transparent().clone())
    }
}

impl TryFrom<&UnifiedKeyStore> for UnifiedFullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        match unified_key_store {
            UnifiedKeyStore::Spend(usk) => Ok(usk.to_unified_full_viewing_key()),
            UnifiedKeyStore::View(ufvk) => Ok(*ufvk.clone()),
            UnifiedKeyStore::Empty => Err(KeyError::NoViewCapability),
        }
    }
}
impl TryFrom<&UnifiedKeyStore> for orchard::keys::FullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.orchard().ok_or(KeyError::NoViewCapability).cloned()
    }
}
impl TryFrom<&UnifiedKeyStore> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.sapling().ok_or(KeyError::NoViewCapability).cloned()
    }
}
impl TryFrom<&UnifiedKeyStore> for zcash_primitives::legacy::keys::AccountPubKey {
    type Error = KeyError;
    fn try_from(unified_key_store: &UnifiedKeyStore) -> Result<Self, Self::Error> {
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        ufvk.transparent()
            .ok_or(KeyError::NoViewCapability)
            .cloned()
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReceiverSelection {
    /// TODO: Add Doc Comment Here!
    pub orchard: bool,
    /// TODO: Add Doc Comment Here!
    pub sapling: bool,
    /// TODO: Add Doc Comment Here!
    pub transparent: bool,
}

impl ReadableWriteable for ReceiverSelection {
    const VERSION: u8 = 1;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let receivers = reader.read_u8()?;
        Ok(Self {
            orchard: receivers & 0b1 != 0,
            sapling: receivers & 0b10 != 0,
            transparent: receivers & 0b100 != 0,
        })
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        let mut receivers = 0;
        if self.orchard {
            receivers |= 0b1;
        };
        if self.sapling {
            receivers |= 0b10;
        };
        if self.transparent {
            receivers |= 0b100;
        };
        writer.write_u8(receivers)?;
        Ok(())
    }
}

#[test]
fn read_write_receiver_selections() {
    for (i, receivers_selected) in (0..8)
        .map(|n| ReceiverSelection::read([1, n].as_slice(), ()).unwrap())
        .enumerate()
    {
        let mut receivers_selected_bytes = [0; 2];
        receivers_selected
            .write(receivers_selected_bytes.as_mut_slice(), ())
            .unwrap();
        assert_eq!(i as u8, receivers_selected_bytes[1]);
    }
}
