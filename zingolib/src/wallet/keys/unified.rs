//! TODO: Add Mod Description Here!

use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    sync::atomic::AtomicBool,
};
use std::{marker::PhantomData, sync::Arc};

use append_only_vec::AppendOnlyVec;
use bip0039::Mnemonic;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_address::unified::{Encoding as _, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_client_backend::wallet::TransparentAddressMetadata;
use zcash_encoding::{CompactSize, Vector};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{NetworkConstants, Parameters};
use zcash_primitives::legacy::TransparentAddress;
use zcash_primitives::zip32::AccountId;

use crate::wallet::error::KeyError;
use crate::wallet::traits::{DomainWalletExt, ReadableWriteable, Recipient};
use crate::{
    config::{ChainType, ZingoConfig},
    wallet::data::new_rejection_address,
};

#[cfg(feature = "ledger-support")]
use super::ledger::LedgerKeys;

use super::legacy::{legacy_sks_to_usk, Capability};
use crate::wallet::keys::capability::{InternalCapability, InMemoryWallet};

pub(crate) const KEY_TYPE_EMPTY: u8 = 0;
pub(crate) const KEY_TYPE_VIEW: u8 = 1;
pub(crate) const KEY_TYPE_SPEND: u8 = 2;

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
    /// Returns true if [`UnifiedKeyStore`] is of `Spend` variant
    pub fn is_spending_key(&self) -> bool {
        matches!(self, UnifiedKeyStore::Spend(_))
    }

    /// Returns true if [`UnifiedKeyStore`] is of `Spend` variant
    pub fn is_empty(&self) -> bool {
        matches!(self, UnifiedKeyStore::Empty)
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
                ))
            }
        })
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        match self {
            UnifiedKeyStore::Spend(usk) => {
                writer.write_u8(KEY_TYPE_SPEND)?;
                usk.write(&mut writer)
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

/// Interface to cryptographic capabilities that the library requires for
/// various operations. <br>
/// It is created either from a [BIP39 mnemonic phrase](<https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki>), <br>
/// loaded from a [`zcash_keys::keys::UnifiedSpendingKey`] <br>
/// or a [`zcash_keys::keys::UnifiedFullViewingKey`]. <br><br>
/// In addition to fundamental spending and viewing keys, the type caches generated addresses.
pub struct WalletCapability {

    #[cfg(feature = "ledger-support")]
    pub(crate) ledger: Option<LedgerKeys>,
    /// Unified key store
    pub unified_key_store: UnifiedKeyStore,
    /// Cache of transparent addresses that the user has created.
    /// Receipts to a single address are correlated on chain.
    /// TODO:  Is there any reason to have this field, apart from the
    /// unified_addresses field?
    pub(crate) transparent_child_addresses: Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
    // TODO: read/write for ephmereral addresses
    // TODO: Remove this field and exclusively use the TxMap field instead
    pub(crate) rejection_addresses: Arc<AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>>,
    /// Cache of unified_addresses
    pub(crate) unified_addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    pub(crate) addresses_write_lock: AtomicBool,
    pub(crate) capability: Box<dyn InternalCapability>,
}
impl Default for WalletCapability {
    fn default() -> Self {
        Self {
            #[cfg(feature = "ledger-support")]
            ledger: None,
            unified_key_store: UnifiedKeyStore::Empty,
            transparent_child_addresses: Arc::new(AppendOnlyVec::new()),
            rejection_addresses: Arc::new(AppendOnlyVec::new()),
            unified_addresses: AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
            capability: Box::new(InMemoryWallet::new()),
        }
    }
}

impl crate::wallet::LightWallet {
    /// This is the interface to expose the wallet key
    pub fn wallet_capability(&self) -> Arc<WalletCapability> {
        self.transaction_context.key.clone()
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

impl WalletCapability {
    #[cfg(feature = "ledger-support")]
    /// checks whether this WalletCapability is a Ledger device or not
    pub fn is_ledger(&self) -> bool {
        self.ledger.is_some()
    }

    pub(crate) fn get_ua_from_contained_transparent_receiver(
        &self,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        self.capability.get_ua_from_contained_transparent_receiver(self, receiver)
    }
    /// TODO: Add Doc Comment Here!
    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        self.capability.addresses(&self)
    }

    /// TODO: Add Doc Comment Here!
    pub fn transparent_child_addresses(&self) -> &Arc<AppendOnlyVec<(usize, TransparentAddress)>> {
        self.capability.transparent_child_addresses(&self)
    }
    /// Generates a unified address from the given desired receivers
    ///
    /// See [`crate::wallet::WalletCapability::generate_transparent_receiver`] for information on using `legacy_key`
    pub fn new_address(
        &self,
        desired_receivers: ReceiverSelection,
        legacy_key: bool,
    ) -> Result<UnifiedAddress, String> {
        self.capability.new_address(&self, desired_receivers, legacy_key)
    }

    /// Generates a transparent receiver for the specified scope.
    pub fn generate_transparent_receiver(
        &self,
        // this should only be `true` when generating transparent addresses while loading from legacy keys (pre wallet version 29)
        // legacy transparent keys are already derived to the external scope so setting `legacy_key` to `true` will skip this scope derivation
        legacy_key: bool,
    ) -> Result<Option<TransparentAddress>, bip32::Error> {
        self.capability.generate_transparent_receiver(&self, legacy_key)
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "not used in zingolib codebase")]
    pub fn get_taddr_to_secretkey_map(
        &self,
        chain: &ChainType,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, KeyError> {
        self.capability.get_taddr_to_secretkey_map(&self, chain)
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_seed(
        config: &ZingoConfig,
        seed: &[u8; 64],
        position: u32,
    ) -> Result<Self, KeyError> {
        let usk = UnifiedSpendingKey::from_seed(
            &config.chain,
            seed,
            AccountId::try_from(position).map_err(KeyError::InvalidAccountId)?,
        )
        .map_err(KeyError::KeyDerivationError)?;

        Ok(Self {
            unified_key_store: UnifiedKeyStore::Spend(Box::new(usk)),
            ..Default::default()
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_phrase(
        config: &ZingoConfig,
        seed_phrase: &Mnemonic,
        position: u32,
    ) -> Result<Self, KeyError> {
        // The seed bytes is the raw entropy. To pass it to HD wallet generation,
        // we need to get the 64 byte bip39 entropy
        let bip39_seed = seed_phrase.to_seed("");
        Self::new_from_seed(config, &bip39_seed, position)
    }

    /// Creates a new `WalletCapability` from a unified spending key.
    pub fn new_from_usk(usk: &[u8]) -> Result<Self, KeyError> {
        // Decode unified spending key
        let usk = UnifiedSpendingKey::from_bytes(Era::Orchard, usk)
            .map_err(|_| KeyError::KeyDecodingError)?;

        Ok(Self {
            unified_key_store: UnifiedKeyStore::Spend(Box::new(usk)),
            ..Default::default()
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_ufvk(config: &ZingoConfig, ufvk_encoded: String) -> Result<Self, KeyError> {
        // Decode UFVK
        if ufvk_encoded.starts_with(config.chain.hrp_sapling_extended_full_viewing_key()) {
            return Err(KeyError::InvalidFormat);
        }
        let (network, ufvk) =
            Ufvk::decode(&ufvk_encoded).map_err(|_| KeyError::KeyDecodingError)?;
        if network != config.chain.network_type() {
            return Err(KeyError::NetworkMismatch);
        }
        let ufvk = UnifiedFullViewingKey::parse(&ufvk).map_err(|_| KeyError::KeyDecodingError)?;

        Ok(Self {
            unified_key_store: UnifiedKeyStore::View(Box::new(ufvk)),
            ..Default::default()
        })
    }

    #[cfg(feature = "ledger-support")]
    /// initializes a new wallet with a ledger 
    pub fn new_with_ledger(config: &ZingoConfig) -> Result<Self, KeyError> {
        use super::ledger::LedgerCapability;

        if !config.use_ledger {
            return Err(KeyError::LedgerNotSet)
        }

        let mut wc = WalletCapability::default();

        wc.ledger = Some(LedgerKeys::new());
        wc.capability = Box::new(LedgerCapability::new());

        Ok(wc)
    }

    /// TODO: This does not appear to be used
    pub(crate) fn get_taddrs(&self, chain: &crate::config::ChainType) -> HashSet<String> {
        self.capability.get_taddrs(&self, chain)
    }

    /// TODO: Add Doc Comment Here!
    pub fn first_sapling_address(&self) -> sapling_crypto::PaymentAddress {
        self.capability.first_sapling_address(&self)
    }

    /// TODO: Add Doc Comment Here!
    //TODO: NAME?????!!
    pub fn get_trees_witness_trees(&self) -> Option<crate::data::witness_trees::WitnessTrees> {
        if self.unified_key_store.is_spending_key() {
            Some(crate::data::witness_trees::WitnessTrees::default())
        } else {
            None
        }
    }

    /// Returns a selection of pools where the wallet can view funds.
    pub fn can_view(&self) -> ReceiverSelection {
        match &self.unified_key_store {
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

impl ReadableWriteable<ChainType, ChainType> for WalletCapability {
    const VERSION: u8 = 5;

    fn read<R: Read>(mut reader: R, input: ChainType) -> io::Result<Self> {
        let version = Self::get_version(&mut reader)?;
        let legacy_key: bool;
        let length_of_rejection_addresses: u32;

        let wc = match version {
            // in version 1, only spending keys are stored
            1 => {
                legacy_key = true;
                length_of_rejection_addresses = 0;

                // Create a temporary USK for address generation to load old wallets
                // due to missing BIP0032 transparent extended private key data
                //
                // USK is re-derived later from seed due to missing BIP0032 transparent extended private key data
                let orchard_sk = orchard::keys::SpendingKey::read(&mut reader, ())?;
                let sapling_sk = sapling_crypto::zip32::ExtendedSpendingKey::read(&mut reader)?;
                let transparent_sk =
                    super::legacy::extended_transparent::ExtendedPrivKey::read(&mut reader, ())?;
                let usk = legacy_sks_to_usk(&orchard_sk, &sapling_sk, &transparent_sk)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                Self {
                    unified_key_store: UnifiedKeyStore::Spend(Box::new(usk)),
                    ..Default::default()
                }
            }
            2 => {
                legacy_key = true;
                length_of_rejection_addresses = 0;

                let orchard_capability = Capability::<
                    orchard::keys::FullViewingKey,
                    orchard::keys::SpendingKey,
                >::read(&mut reader, ())?;
                let sapling_capability = Capability::<
                    sapling_crypto::zip32::DiversifiableFullViewingKey,
                    sapling_crypto::zip32::ExtendedSpendingKey,
                >::read(&mut reader, ())?;
                let transparent_capability = Capability::<
                    super::legacy::extended_transparent::ExtendedPubKey,
                    super::legacy::extended_transparent::ExtendedPrivKey,
                >::read(&mut reader, ())?;

                let orchard_fvk = match &orchard_capability {
                    Capability::View(fvk) => Some(fvk),
                    _ => None,
                };
                let sapling_fvk = match &sapling_capability {
                    Capability::View(fvk) => Some(fvk),
                    _ => None,
                };
                let transparent_fvk = match &transparent_capability {
                    Capability::View(fvk) => Some(fvk),
                    _ => None,
                };

                let unified_key_store = if orchard_fvk.is_some()
                    || sapling_fvk.is_some()
                    || transparent_fvk.is_some()
                {
                    // In the case of loading from viewing keys:
                    // Create the UFVK from FVKs.
                    let ufvk = super::legacy::legacy_fvks_to_ufvk(
                        orchard_fvk,
                        sapling_fvk,
                        transparent_fvk,
                        &input,
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                    UnifiedKeyStore::View(Box::new(ufvk))
                } else if matches!(sapling_capability.clone(), Capability::Spend(_)) {
                    // In the case of loading spending keys:
                    // Only sapling is checked for spend capability due to only supporting a full set of spend keys
                    //
                    // Create a temporary USK for address generation to load old wallets
                    // due to missing BIP0032 transparent extended private key data
                    //
                    // USK is re-derived later from seed due to missing BIP0032 transparent extended private key data
                    // this missing data is not required for UFVKs
                    let orchard_sk = match &orchard_capability {
                        Capability::Spend(sk) => sk,
                        _ => return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Orchard spending key not found. Wallet should have full spend capability!"
                                .to_string(),
                        )),
                    };
                    let sapling_sk = match &sapling_capability {
                        Capability::Spend(sk) => sk,
                        _ => return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Sapling spending key not found. Wallet should have full spend capability!"
                                .to_string(),
                        )),
                    };
                    let transparent_sk = match &transparent_capability {
                        Capability::Spend(sk) => sk,
                        _ => return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Transparent spending key not found. Wallet should have full spend capability!"
                                .to_string(),
                        )),
                    };

                    let usk = legacy_sks_to_usk(orchard_sk, sapling_sk, transparent_sk)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

                    UnifiedKeyStore::Spend(Box::new(usk))
                } else {
                    UnifiedKeyStore::Empty
                };

                Self {
                    unified_key_store,
                    ..Default::default()
                }
            }
            3 => {
                legacy_key = false;
                length_of_rejection_addresses = 0;

                Self {
                    unified_key_store: UnifiedKeyStore::read(&mut reader, input)?,
                    ..Default::default()
                }
            }
            4 => {
                legacy_key = false;
                length_of_rejection_addresses = reader.read_u32::<LittleEndian>()?;

                Self {
                    unified_key_store: UnifiedKeyStore::read(&mut reader, input)?,
                    ..Default::default()
                }
            }
            5 => {
                legacy_key = false;
                length_of_rejection_addresses = reader.read_u32::<LittleEndian>()?;

                Self {
                    unified_key_store: UnifiedKeyStore::read(&mut reader, input)?,
                    ..Default::default()
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid WalletCapability version".to_string(),
                ))
            }
        };
        let receiver_selections = Vector::read(&mut reader, |r| ReceiverSelection::read(r, ()))?;
        for rs in receiver_selections {
            wc.new_address(rs, legacy_key)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }

        for _ in 0..length_of_rejection_addresses {
            new_rejection_address(
                &wc.rejection_addresses,
                &wc.rejection_ivk()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }

        Ok(wc)
    }

    fn write<W: Write>(&self, mut writer: W, input: ChainType) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        writer.write_u32::<LittleEndian>(self.rejection_addresses.len() as u32)?;
        self.unified_key_store.write(&mut writer, input)?;
        Vector::write(
            &mut writer,
            &self.unified_addresses.iter().collect::<Vec<_>>(),
            |w, address| {
                ReceiverSelection {
                    orchard: address.orchard().is_some(),
                    sapling: address.sapling().is_some(),
                    transparent: address.transparent().is_some(),
                }
                .write(w, ())
            },
        )
    }
}

/// The external, default scope for deriving an fvk's component viewing keys
pub struct External;

/// The internal scope, used for change only
pub struct Internal;

mod scope {
    use super::*;
    use zcash_primitives::zip32::Scope as ScopeEnum;
    pub trait Scope {
        fn scope() -> ScopeEnum;
    }

    impl Scope for External {
        fn scope() -> ScopeEnum {
            ScopeEnum::External
        }
    }
    impl Scope for Internal {
        fn scope() -> ScopeEnum {
            ScopeEnum::Internal
        }
    }
}

/// TODO: Add Doc Comment Here!
pub struct Ivk<D, Scope>
where
    D: zcash_note_encryption::Domain,
{
    /// TODO: Add Doc Comment Here!
    pub ivk: D::IncomingViewingKey,
    __scope: PhantomData<Scope>,
}

/// This is of questionable utility, but internally-scoped ovks
/// exist, and so we represent them at the type level despite
/// having no current use for them
pub struct Ovk<D, Scope>
where
    D: zcash_note_encryption::Domain,
{
    /// TODO: Add Doc Comment Here!
    pub ovk: D::OutgoingViewingKey,
    __scope: PhantomData<Scope>,
}

/// TODO: Add Doc Comment Here!
pub trait Fvk<D: DomainWalletExt>
where
    <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
    <D as zcash_note_encryption::Domain>::Recipient: Recipient,
{
    /// TODO: Add Doc Comment Here!
    fn derive_ivk<S: scope::Scope>(&self) -> Ivk<D, S>;
    /// TODO: Add Doc Comment Here!
    fn derive_ovk<S: scope::Scope>(&self) -> Ovk<D, S>;
}

impl Fvk<OrchardDomain> for orchard::keys::FullViewingKey {
    fn derive_ivk<S: scope::Scope>(&self) -> Ivk<OrchardDomain, S> {
        Ivk {
            ivk: orchard::keys::PreparedIncomingViewingKey::new(&self.to_ivk(S::scope())),
            __scope: PhantomData,
        }
    }

    fn derive_ovk<S: scope::Scope>(&self) -> Ovk<OrchardDomain, S> {
        Ovk {
            ovk: self.to_ovk(S::scope()),
            __scope: PhantomData,
        }
    }
}

impl Fvk<SaplingDomain> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    fn derive_ivk<S: scope::Scope>(&self) -> Ivk<SaplingDomain, S> {
        Ivk {
            ivk: sapling_crypto::keys::PreparedIncomingViewingKey::new(&self.to_ivk(S::scope())),
            __scope: PhantomData,
        }
    }

    fn derive_ovk<S: scope::Scope>(&self) -> Ovk<SaplingDomain, S> {
        Ovk {
            ovk: self.to_ovk(S::scope()),
            __scope: PhantomData,
        }
    }
}
mod rejection {
    use std::{collections::HashSet, sync::Arc};

    use append_only_vec::AppendOnlyVec;
    use zcash_client_backend::wallet::TransparentAddressMetadata;
    use zcash_keys::{encoding::AddressCodec, keys::DerivationError};
    use zcash_primitives::legacy::{
        keys::{AccountPubKey, NonHardenedChildIndex, TransparentKeyScope},
        TransparentAddress,
    };

    use crate::wallet::error::KeyError;

    use super::WalletCapability;

    impl WalletCapability {
        pub(crate) fn rejection_ivk(
            &self,
        ) -> Result<zcash_primitives::legacy::keys::EphemeralIvk, KeyError> {
            AccountPubKey::try_from(&self.unified_key_store)?
                .derive_ephemeral_ivk()
                .map_err(DerivationError::Transparent)
                .map_err(KeyError::KeyDerivationError)
        }
        pub(crate) fn get_rejection_address_by_index(
            rejection_ivk: &zcash_primitives::legacy::keys::EphemeralIvk,
            rejection_address_index: u32,
        ) -> Result<(TransparentAddress, TransparentAddressMetadata), KeyError> {
            let address_index = NonHardenedChildIndex::from_index(rejection_address_index)
                .ok_or(KeyError::InvalidNonHardenedChildIndex)?;
            Ok((
                rejection_ivk
                    .derive_ephemeral_address(address_index)
                    .map_err(DerivationError::Transparent)
                    .map_err(KeyError::KeyDerivationError)?,
                TransparentAddressMetadata::new(TransparentKeyScope::EPHEMERAL, address_index),
            ))
        }
        /// TODO: Add Doc Comment Here!
        pub fn get_rejection_addresses(
            &self,
        ) -> &Arc<AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>> {
            &self.rejection_addresses
        }
        pub(crate) fn get_rejection_address_set(
            &self,
            chain: &crate::config::ChainType,
        ) -> HashSet<String> {
            self.rejection_addresses
                .iter()
                .map(|(transparent_address, _metadata)| transparent_address.encode(chain))
                .collect()
        }
    }
}
