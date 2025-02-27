//! Module for legacy code associated with wallet keys required for backward-compatility with old wallet versions

use std::{
    io::{self, Read, Write},
    marker::PhantomData,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use append_only_vec::AppendOnlyVec;
use bip0039::Mnemonic;
use bip32::ExtendedPublicKey;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use zcash_address::unified::Typecode;
use zcash_client_backend::wallet::TransparentAddressMetadata;
use zcash_encoding::{CompactSize, Vector};
use zcash_keys::{
    address::UnifiedAddress,
    keys::{DerivationError, Era, UnifiedFullViewingKey, UnifiedSpendingKey},
};
use zcash_primitives::legacy::{
    keys::{AccountPubKey, IncomingViewingKey as _, NonHardenedChildIndex, TransparentKeyScope},
    TransparentAddress,
};
use zip32::{AccountId, DiversifierIndex};

use crate::{
    config::{ChainType, ZingoConfig},
    wallet::{error::KeyError, traits::ReadableWriteable},
};

use super::unified::{
    ReceiverSelection, UnifiedKeyStore, KEY_TYPE_EMPTY, KEY_TYPE_SPEND, KEY_TYPE_VIEW,
};

pub mod extended_transparent;

/// Interface to cryptographic capabilities that the library requires for
/// various operations. <br>
/// It is created either from a [BIP39 mnemonic phrase](<https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki>), <br>
/// loaded from a [`zcash_keys::keys::UnifiedSpendingKey`] <br>
/// or a [`zcash_keys::keys::UnifiedFullViewingKey`]. <br><br>
/// In addition to fundamental spending and viewing keys, the type caches generated addresses.
pub struct WalletCapability {
    /// Unified key store
    pub unified_key_store: UnifiedKeyStore,
    /// Cache of transparent addresses that the user has created.
    /// Receipts to a single address are correlated on chain.
    /// TODO:  Is there any reason to have this field, apart from the
    /// unified_addresses field?
    transparent_child_addresses: Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
    // TODO: read/write for ephmereral addresses
    // TODO: Remove this field and exclusively use the TxMap field instead
    rejection_addresses: Arc<AppendOnlyVec<(TransparentAddress, TransparentAddressMetadata)>>,
    /// Cache of unified_addresses
    unified_addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    addresses_write_lock: AtomicBool,
}
impl Default for WalletCapability {
    fn default() -> Self {
        Self {
            unified_key_store: UnifiedKeyStore::Empty,
            transparent_child_addresses: Arc::new(AppendOnlyVec::new()),
            rejection_addresses: Arc::new(AppendOnlyVec::new()),
            unified_addresses: AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
        }
    }
}

impl WalletCapability {
    /// TODO: Add Doc Comment Here!
    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        &self.unified_addresses
    }

    /// TODO: Add Doc Comment Here!
    pub fn transparent_child_addresses(&self) -> &Arc<AppendOnlyVec<(usize, TransparentAddress)>> {
        &self.transparent_child_addresses
    }

    /// Generates a unified address from the given desired receivers
    ///
    /// See [`self::WalletCapability::generate_transparent_receiver`] for information on using `legacy_key`
    pub fn new_address(
        &self,
        desired_receivers: ReceiverSelection,
        legacy_key: bool,
    ) -> Result<UnifiedAddress, String> {
        if self
            .addresses_write_lock
            .swap(true, atomic::Ordering::Acquire)
        {
            return Err("addresses_write_lock collision!".to_string());
        }

        let previous_num_addresses = self.unified_addresses.len();
        let orchard_receiver = if desired_receivers.orchard {
            let fvk: orchard::keys::FullViewingKey = match (&self.unified_key_store).try_into() {
                Ok(viewkey) => viewkey,
                Err(e) => {
                    self.addresses_write_lock
                        .swap(false, atomic::Ordering::Release);
                    return Err(e.to_string());
                }
            };
            Some(fvk.address_at(self.unified_addresses.len(), orchard::keys::Scope::External))
        } else {
            None
        };

        // produce a Sapling address to increment Sapling diversifier index
        let sapling_receiver = if desired_receivers.sapling {
            let mut sapling_diversifier_index = DiversifierIndex::new();
            let mut address;
            let mut count = 0;
            let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey =
                match (&self.unified_key_store).try_into() {
                    Ok(viewkey) => viewkey,
                    Err(e) => {
                        self.addresses_write_lock
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
                if count == self.unified_addresses.len() {
                    break;
                }
                count += 1;
            }
            Some(address)
        } else {
            None
        };

        let transparent_receiver = if desired_receivers.transparent {
            self.generate_transparent_receiver(legacy_key)
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
                self.addresses_write_lock
                    .swap(false, atomic::Ordering::Release);
                return Err(
                    "Invalid receivers requested! At least one of sapling or orchard required"
                        .to_string(),
                );
            }
        };
        self.unified_addresses.push(ua.clone());
        assert_eq!(self.unified_addresses.len(), previous_num_addresses + 1);
        self.addresses_write_lock
            .swap(false, atomic::Ordering::Release);
        Ok(ua)
    }

    /// Generates a transparent receiver for the specified scope.
    pub fn generate_transparent_receiver(
        &self,
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

            self.transparent_child_addresses
                .push((self.addresses().len(), t_addr));
            Ok(t_addr)
        };
        let child_index = NonHardenedChildIndex::from_index(self.addresses().len() as u32)
            .expect("hardened bit should not be set for non-hardened child indexes");
        let transparent_receiver = match &self.unified_key_store {
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

    /// TODO: Add Doc Comment Here!
    pub fn first_sapling_address(&self) -> sapling_crypto::PaymentAddress {
        // This index is dangerous, but all ways to instantiate a UnifiedSpendAuthority
        // create it with a suitable first address
        *self.addresses()[0].sapling().unwrap()
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
}

impl ReadableWriteable<ChainType, ChainType> for WalletCapability {
    const VERSION: u8 = 4;

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

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Capability<ViewingKeyType, SpendKeyType> {
    /// TODO: Add Doc Comment Here!
    None,
    /// TODO: Add Doc Comment Here!
    View(ViewingKeyType),
    /// TODO: Add Doc Comment Here!
    Spend(SpendKeyType),
}

impl<V, S> ReadableWriteable<(), ()> for Capability<V, S>
where
    V: ReadableWriteable<(), ()>,
    S: ReadableWriteable<(), ()>,
{
    const VERSION: u8 = 1;
    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let capability_type = reader.read_u8()?;
        Ok(match capability_type {
            KEY_TYPE_EMPTY => Capability::None,
            KEY_TYPE_VIEW => Capability::View(V::read(&mut reader, ())?),
            KEY_TYPE_SPEND => Capability::Spend(S::read(&mut reader, ())?),
            x => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unknown wallet Capability type: {}", x),
                ))
            }
        })
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        match self {
            Capability::None => writer.write_u8(KEY_TYPE_EMPTY),
            Capability::View(vk) => {
                writer.write_u8(KEY_TYPE_VIEW)?;
                vk.write(&mut writer, ())
            }
            Capability::Spend(sk) => {
                writer.write_u8(KEY_TYPE_SPEND)?;
                sk.write(&mut writer, ())
            }
        }
    }
}

pub(crate) fn legacy_fvks_to_ufvk<P: zcash_primitives::consensus::Parameters>(
    orchard_fvk: Option<&orchard::keys::FullViewingKey>,
    sapling_fvk: Option<&sapling_crypto::zip32::DiversifiableFullViewingKey>,
    transparent_fvk: Option<&extended_transparent::ExtendedPubKey>,
    parameters: &P,
) -> Result<UnifiedFullViewingKey, KeyError> {
    use zcash_address::unified::Encoding;

    let mut fvks = Vec::new();
    if let Some(fvk) = orchard_fvk {
        fvks.push(zcash_address::unified::Fvk::Orchard(fvk.to_bytes()));
    }
    if let Some(fvk) = sapling_fvk {
        fvks.push(zcash_address::unified::Fvk::Sapling(fvk.to_bytes()));
    }
    if let Some(fvk) = transparent_fvk {
        let mut fvk_bytes = [0u8; 65];
        fvk_bytes[0..32].copy_from_slice(&fvk.chain_code[..]);
        fvk_bytes[32..65].copy_from_slice(&fvk.public_key.serialize()[..]);
        fvks.push(zcash_address::unified::Fvk::P2pkh(fvk_bytes));
    }

    let ufvk = zcash_address::unified::Ufvk::try_from_items(fvks)?;

    UnifiedFullViewingKey::decode(parameters, &ufvk.encode(&parameters.network_type()))
        .map_err(|_| KeyError::KeyDecodingError)
}

pub(crate) fn legacy_sks_to_usk(
    orchard_key: &orchard::keys::SpendingKey,
    sapling_key: &sapling_crypto::zip32::ExtendedSpendingKey,
    transparent_key: &extended_transparent::ExtendedPrivKey,
) -> Result<UnifiedSpendingKey, KeyError> {
    let mut usk_bytes = vec![];

    // hard-coded Orchard Era ID due to `id()` being a private fn
    usk_bytes.write_u32::<LittleEndian>(0xc2d6_d0b4)?;

    CompactSize::write(
        &mut usk_bytes,
        usize::try_from(Typecode::Orchard).expect("typecode to usize should not fail"),
    )?;
    let orchard_key_bytes = orchard_key.to_bytes();
    CompactSize::write(&mut usk_bytes, orchard_key_bytes.len())?;
    usk_bytes.write_all(orchard_key_bytes)?;

    CompactSize::write(
        &mut usk_bytes,
        usize::try_from(Typecode::Sapling).expect("typecode to usize should not fail"),
    )?;
    let sapling_key_bytes = sapling_key.to_bytes();
    CompactSize::write(&mut usk_bytes, sapling_key_bytes.len())?;
    usk_bytes.write_all(&sapling_key_bytes)?;

    // the following code performs the same operations for calling `to_bytes()` on an AccountPrivKey in LRZ
    let prefix = bip32::Prefix::XPRV;
    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(&transparent_key.chain_code);
    let attrs = bip32::ExtendedKeyAttrs {
        depth: 4,
        parent_fingerprint: [0xff, 0xff, 0xff, 0xff],
        child_number: bip32::ChildNumber::new(0, true).expect("correct"),
        chain_code,
    };
    // Add leading `0` byte
    let mut key_bytes = [0u8; 33];
    key_bytes[1..].copy_from_slice(transparent_key.private_key.as_ref());

    let extended_key = bip32::ExtendedKey {
        prefix,
        attrs,
        key_bytes,
    };

    let xprv_encoded = extended_key.to_string();
    let account_tkey_bytes = bs58::decode(xprv_encoded)
        .with_check(None)
        .into_vec()
        .expect("correct")
        .split_off(bip32::Prefix::LENGTH);

    CompactSize::write(
        &mut usk_bytes,
        usize::try_from(Typecode::P2pkh).expect("typecode to usize should not fail"),
    )?;
    CompactSize::write(&mut usk_bytes, account_tkey_bytes.len())?;
    usk_bytes.write_all(&account_tkey_bytes)?;

    UnifiedSpendingKey::from_bytes(Era::Orchard, &usk_bytes).map_err(|_| KeyError::KeyDecodingError)
}

/// Generates a transparent address from legacy key
///
/// Legacy key is a key used ONLY during wallet load for wallet versions <29
/// This legacy key is already derived to the external scope so should only derive a child at the `address_index`
/// and use this child to derive the transparent address
#[allow(deprecated)]
pub(crate) fn generate_transparent_address_from_legacy_key(
    external_pubkey: &AccountPubKey,
    address_index: NonHardenedChildIndex,
) -> Result<TransparentAddress, bip32::Error> {
    let external_pubkey_bytes = external_pubkey.serialize();

    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(&external_pubkey_bytes[..32]);
    let public_key = secp256k1::PublicKey::from_slice(&external_pubkey_bytes[32..])?;

    let extended_pubkey = ExtendedPublicKey::new(
        public_key,
        bip32::ExtendedKeyAttrs {
            depth: 4,
            parent_fingerprint: [0xff, 0xff, 0xff, 0xff],
            child_number: bip32::ChildNumber::new(0, true)
                .expect("hard-coded index of 0 is not larger than the hardened bit"),
            chain_code,
        },
    );

    // address generation copied from IncomingViewingKey::derive_address in LRZ
    let child_key = extended_pubkey.derive_child(address_index.into())?;
    Ok(zcash_primitives::legacy::keys::pubkey_to_address(
        child_key.public_key(),
    ))
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
