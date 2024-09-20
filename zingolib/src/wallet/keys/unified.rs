//! TODO: Add Mod Discription Here!
use std::sync::atomic;
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
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{BranchId, NetworkConstants, Parameters};
use zcash_primitives::legacy::keys::{IncomingViewingKey, NonHardenedChildIndex};

use crate::config::{ChainType, ZingoConfig};
use crate::wallet::error::KeyError;
use secp256k1::SecretKey;
use zcash_address::unified::{Encoding, Typecode, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_encoding::{CompactSize, Vector};
use zcash_primitives::zip32::AccountId;
use zcash_primitives::{legacy::TransparentAddress, zip32::DiversifierIndex};

use crate::wallet::traits::{DomainWalletExt, ReadableWriteable, Recipient};

use super::{extended_transparent::ExtendedPrivKey, ToBase58Check};

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

impl<V, S> Capability<V, S> {
    /// TODO: Add Doc Comment Here!
    pub fn can_spend(&self) -> bool {
        matches!(self, Capability::Spend(_))
    }

    /// TODO: Add Doc Comment Here!
    pub fn can_view(&self) -> bool {
        match self {
            Capability::None => false,
            Capability::View(_) => true,
            Capability::Spend(_) => true,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn kind_str(&self) -> &'static str {
        match self {
            Capability::None => "No key",
            Capability::View(_) => "View only",
            Capability::Spend(_) => "Spend capable",
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub struct WalletCapability {
    /// TODO: Add Doc Comment Here!
    pub transparent: Capability<
        zcash_primitives::legacy::keys::AccountPubKey,
        zcash_primitives::legacy::keys::AccountPrivKey,
    >,
    /// TODO: Add Doc Comment Here!
    pub sapling: Capability<
        sapling_crypto::zip32::DiversifiableFullViewingKey,
        sapling_crypto::zip32::ExtendedSpendingKey,
    >,
    /// TODO: Add Doc Comment Here!
    pub orchard: Capability<orchard::keys::FullViewingKey, orchard::keys::SpendingKey>,

    transparent_child_addresses: Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,
    addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    // Not all diversifier indexes produce valid sapling addresses.
    // Because of this, the index isn't necessarily equal to addresses.len()
    addresses_write_lock: AtomicBool,
}
impl Default for WalletCapability {
    fn default() -> Self {
        Self {
            orchard: Capability::None,
            sapling: Capability::None,
            transparent: Capability::None,
            transparent_child_addresses: Arc::new(AppendOnlyVec::new()),
            addresses: AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
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
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ReceiverSelection {
    /// TODO: Add Doc Comment Here!
    pub orchard: bool,
    /// TODO: Add Doc Comment Here!
    pub sapling: bool,
    /// TODO: Add Doc Comment Here!
    pub transparent: bool,
}

impl ReadableWriteable<()> for ReceiverSelection {
    const VERSION: u8 = 1;

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let receivers = reader.read_u8()?;
        Ok(Self {
            orchard: receivers & 0b1 != 0,
            sapling: receivers & 0b10 != 0,
            transparent: receivers & 0b100 != 0,
        })
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
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
            .write(receivers_selected_bytes.as_mut_slice())
            .unwrap();
        assert_eq!(i as u8, receivers_selected_bytes[1]);
    }
}

impl WalletCapability {
    pub(crate) fn get_ua_from_contained_transparent_receiver(
        &self,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        self.addresses
            .iter()
            .find(|ua| ua.transparent() == Some(receiver))
            .cloned()
    }
    /// TODO: Add Doc Comment Here!
    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        &self.addresses
    }

    /// TODO: Add Doc Comment Here!
    pub fn transparent_child_addresses(&self) -> &Arc<AppendOnlyVec<(usize, TransparentAddress)>> {
        &self.transparent_child_addresses
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_address(
        &self,
        desired_receivers: ReceiverSelection,
    ) -> Result<UnifiedAddress, String> {
        if (desired_receivers.transparent & !self.transparent.can_view())
            | (desired_receivers.sapling & !self.sapling.can_view()
                | (desired_receivers.orchard & !self.orchard.can_view()))
        {
            return Err("The wallet is not capable of producing desired receivers.".to_string());
        }
        if self
            .addresses_write_lock
            .swap(true, atomic::Ordering::Acquire)
        {
            return Err("addresses_write_lock collision!".to_string());
        }
        let previous_num_addresses = self.addresses.len();
        let orchard_receiver = if desired_receivers.orchard {
            let fvk: orchard::keys::FullViewingKey = match self.try_into() {
                Ok(viewkey) => viewkey,
                Err(e) => {
                    self.addresses_write_lock
                        .swap(false, atomic::Ordering::Release);
                    return Err(e.to_string());
                }
            };
            Some(fvk.address_at(self.addresses.len(), orchard::keys::Scope::External))
        } else {
            None
        };

        // produce a Sapling address to increment Sapling diversifier index
        let sapling_receiver = if desired_receivers.sapling && self.sapling.can_view() {
            let mut sapling_diversifier_index = DiversifierIndex::new();
            let mut address;
            let mut count = 0;
            let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey =
                self.try_into().expect("to create an fvk");
            loop {
                (sapling_diversifier_index, address) = fvk
                    .find_address(sapling_diversifier_index)
                    .expect("Diversifier index overflow");
                sapling_diversifier_index
                    .increment()
                    .expect("diversifier index overflow");
                if count == self.addresses.len() {
                    break;
                }
                count += 1;
            }
            Some(address)
        } else {
            None
        };

        let transparent_receiver = if desired_receivers.transparent {
            let child_index = NonHardenedChildIndex::from_index(self.addresses.len() as u32)
                .expect("hardened bit should not be set for non-hardened child indexes");
            let external_pubkey = match &self.transparent {
                Capability::Spend(private_key) => {
                    private_key.to_account_pubkey().derive_external_ivk().ok()
                }
                Capability::View(public_key) => public_key.derive_external_ivk().ok(),
                Capability::None => None,
            };
            if let Some(pk) = external_pubkey {
                let t_addr = pk.derive_address(child_index).unwrap();
                self.transparent_child_addresses
                    .push((self.addresses.len(), t_addr));
                Some(t_addr)
            } else {
                None
            }
        } else {
            None
        };

        let ua = UnifiedAddress::from_receivers(
            orchard_receiver,
            sapling_receiver,
            #[allow(deprecated)]
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
        self.addresses.push(ua.clone());
        assert_eq!(self.addresses.len(), previous_num_addresses + 1);
        self.addresses_write_lock
            .swap(false, atomic::Ordering::Release);
        Ok(ua)
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_taddr_to_secretkey_map(
        &self,
        chain: &ChainType,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, KeyError> {
        if let Capability::Spend(transparent_sk) = &self.transparent {
            self.transparent_child_addresses()
                .iter()
                .map(|(i, taddr)| -> Result<_, KeyError> {
                    let hash = match taddr {
                        TransparentAddress::PublicKeyHash(hash) => hash,
                        TransparentAddress::ScriptHash(hash) => hash,
                    };
                    Ok((
                        hash.to_base58check(&chain.b58_pubkey_address_prefix(), &[]),
                        transparent_sk
                            .derive_external_secret_key(
                                NonHardenedChildIndex::from_index(*i as u32)
                                    .ok_or(KeyError::InvalidNonHardenedChildIndex)?,
                            )
                            .map_err(|_| KeyError::KeyDerivationError)?,
                    ))
                })
                .collect::<Result<_, _>>()
        } else {
            Err(KeyError::NoSpendCapability)
        }
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
        .map_err(|_| KeyError::KeyDerivationError)?;

        Ok(Self {
            orchard: Capability::Spend(usk.orchard().clone()),
            sapling: Capability::Spend(usk.sapling().clone()),
            transparent: Capability::Spend(usk.transparent().clone()),
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
            orchard: Capability::Spend(usk.orchard().clone()),
            sapling: Capability::Spend(usk.sapling().clone()),
            transparent: Capability::Spend(usk.transparent().clone()),
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
        let ufvk = UnifiedFullViewingKey::parse(&ufvk).unwrap();

        Ok(Self {
            orchard: Capability::View(ufvk.orchard().ok_or(KeyError::MissingViewingKey)?.clone()),
            sapling: Capability::View(ufvk.sapling().ok_or(KeyError::MissingViewingKey)?.clone()),
            transparent: Capability::View(
                ufvk.transparent()
                    .ok_or(KeyError::MissingViewingKey)?
                    .clone(),
            ),
            ..Default::default()
        })
    }

    pub(crate) fn get_all_taddrs(&self, chain: &crate::config::ChainType) -> HashSet<String> {
        self.addresses
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

    /// TODO: Add Doc Comment Here!
    pub fn first_sapling_address(&self) -> sapling_crypto::PaymentAddress {
        // This index is dangerous, but all ways to instantiate a UnifiedSpendAuthority
        // create it with a suitable first address
        *self.addresses()[0].sapling().unwrap()
    }

    /// Returns a selection of pools where the wallet can spend funds.
    pub fn can_spend_from_all_pools(&self) -> bool {
        self.orchard.can_spend() && self.sapling.can_spend() && self.transparent.can_spend()
    }

    /// TODO: Add Doc Comment Here!
    //TODO: NAME?????!!
    pub fn get_trees_witness_trees(&self) -> Option<crate::data::witness_trees::WitnessTrees> {
        if self.can_spend_from_all_pools() {
            Some(crate::data::witness_trees::WitnessTrees::default())
        } else {
            None
        }
    }

    /// Returns a selection of pools where the wallet can view funds.
    pub fn can_view(&self) -> ReceiverSelection {
        ReceiverSelection {
            orchard: self.orchard.can_view(),
            sapling: self.sapling.can_view(),
            transparent: self.transparent.can_view(),
        }
    }
}

/// Reads a transparent ExtendedPrivKey from a buffer that has a 32 byte private key and 32 byte chain code.
fn transparent_key_from_bytes(bytes: &[u8]) -> Result<ExtendedPrivKey, std::io::Error> {
    let mut reader = std::io::Cursor::new(bytes);

    let private_key = SecretKey::read(&mut reader, ())?;
    let mut chain_code = [0; 32];
    reader.read_exact(&mut chain_code)?;

    Ok(ExtendedPrivKey {
        chain_code: chain_code.to_vec(),
        private_key,
    })
}

impl<V, S> ReadableWriteable<()> for Capability<V, S>
where
    V: ReadableWriteable<()>,
    S: ReadableWriteable<()>,
{
    const VERSION: u8 = 1;
    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let capability_type = reader.read_u8()?;
        Ok(match capability_type {
            0 => Capability::None,
            1 => Capability::View(V::read(&mut reader, ())?),
            2 => Capability::Spend(S::read(&mut reader, ())?),
            x => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unknown wallet Capability type: {}", x),
                ))
            }
        })
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        match self {
            Capability::None => writer.write_u8(0),
            Capability::View(vk) => {
                writer.write_u8(1)?;
                vk.write(&mut writer)
            }
            Capability::Spend(sk) => {
                writer.write_u8(2)?;
                sk.write(&mut writer)
            }
        }
    }
}

impl ReadableWriteable<()> for WalletCapability {
    const VERSION: u8 = 2;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let version = Self::get_version(&mut reader)?;
        let wc = match version {
            // in version 1, only spending keys are stored
            1 => {
                let orchard = orchard::keys::SpendingKey::read(&mut reader, ())?;
                let sapling = sapling_crypto::zip32::ExtendedSpendingKey::read(&mut reader)?;
                let transparent =
                    super::extended_transparent::ExtendedPrivKey::read(&mut reader, ())?;
                Self {
                    orchard: Capability::Spend(orchard),
                    sapling: Capability::Spend(sapling),
                    transparent: Capability::Spend(transparent),
                    ..Default::default()
                }
            }
            2 => Self {
                orchard: Capability::read(&mut reader, ())?,
                sapling: Capability::read(&mut reader, ())?,
                transparent: Capability::read(&mut reader, ())?,
                ..Default::default()
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid WalletCapability version".to_string(),
                ))
            }
        };
        let receiver_selections = Vector::read(reader, |r| ReceiverSelection::read(r, ()))?;
        for rs in receiver_selections {
            wc.new_address(rs)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }
        Ok(wc)
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        self.orchard.write(&mut writer)?;
        self.sapling.write(&mut writer)?;
        self.transparent.write(&mut writer)?;
        Vector::write(
            &mut writer,
            &self.addresses.iter().collect::<Vec<_>>(),
            |w, address| {
                ReceiverSelection {
                    orchard: address.orchard().is_some(),
                    sapling: address.sapling().is_some(),
                    transparent: address.transparent().is_some(),
                }
                .write(w)
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

impl TryFrom<&WalletCapability> for zcash_primitives::legacy::keys::AccountPrivKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, Self::Error> {
        match &wc.transparent {
            Capability::Spend(sk) => Ok(sk.clone()),
            _ => Err(KeyError::NoSpendCapability),
        }
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        match &wc.sapling {
            Capability::Spend(sk) => Ok(sk.clone()),
            _ => Err(KeyError::NoSpendCapability),
        }
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::SpendingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        match &wc.orchard {
            Capability::Spend(sk) => Ok(*sk),
            _ => Err(KeyError::NoSpendCapability),
        }
    }
}

impl TryFrom<&WalletCapability> for zcash_primitives::legacy::keys::AccountPubKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, Self::Error> {
        match &wc.transparent {
            Capability::Spend(priv_key) => Ok(priv_key.to_account_pubkey()),
            Capability::View(pub_key) => Ok(pub_key.clone()),
            Capability::None => Err(KeyError::NoViewCapability),
        }
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::FullViewingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        match &wc.orchard {
            Capability::Spend(sk) => Ok(orchard::keys::FullViewingKey::from(sk)),
            Capability::View(fvk) => Ok(fvk.clone()),
            Capability::None => Err(KeyError::NoViewCapability),
        }
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        match &wc.sapling {
            Capability::Spend(sk) => {
                let dfvk = sk.to_diversifiable_full_viewing_key();
                Ok(dfvk)
            }
            Capability::View(fvk) => Ok(fvk.clone()),
            Capability::None => Err(KeyError::NoViewCapability),
        }
    }
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

impl TryFrom<&WalletCapability> for orchard::keys::OutgoingViewingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
        Ok(fvk.to_ovk(orchard::keys::Scope::External))
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::keys::OutgoingViewingKey {
    type Error = KeyError;
    fn try_from(wc: &WalletCapability) -> Result<Self, KeyError> {
        let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey = wc.try_into()?;
        Ok(fvk.fvk().ovk)
    }
}

impl TryFrom<&WalletCapability> for UnifiedSpendingKey {
    type Error = io::Error;

    fn try_from(value: &WalletCapability) -> Result<Self, Self::Error> {
        let transparent = &value.transparent;
        let sapling = &value.sapling;
        let orchard = &value.orchard;
        match (transparent, sapling, orchard) {
            (Capability::Spend(tkey), Capability::Spend(skey), Capability::Spend(okey)) => {
                let mut key_bytes = Vec::new();
                // orchard Era usk
                key_bytes.write_u32::<LittleEndian>(BranchId::Nu5.into())?;

                let okey_bytes = okey.to_bytes();
                CompactSize::write(&mut key_bytes, u32::from(Typecode::Orchard) as usize)?;
                CompactSize::write(&mut key_bytes, okey_bytes.len())?;
                key_bytes.write_all(okey_bytes)?;

                let skey_bytes = skey.to_bytes();
                CompactSize::write(&mut key_bytes, u32::from(Typecode::Sapling) as usize)?;
                CompactSize::write(&mut key_bytes, skey_bytes.len())?;
                key_bytes.write_all(&skey_bytes)?;

                let mut tkey_bytes = Vec::new();
                tkey_bytes.write_all(tkey.private_key.as_ref())?;
                tkey_bytes.write_all(&tkey.chain_code)?;

                CompactSize::write(&mut key_bytes, u32::from(Typecode::P2pkh) as usize)?;
                CompactSize::write(&mut key_bytes, tkey_bytes.len())?;
                key_bytes.write_all(&tkey_bytes)?;

                UnifiedSpendingKey::from_bytes(Era::Orchard, &key_bytes).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidInput, format!("bad usk: {e}"))
                })
            }
            _otherwise => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "don't have spend keys",
            )),
        }
    }
}
