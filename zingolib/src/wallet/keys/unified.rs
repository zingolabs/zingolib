//! TODO: Add Mod Discription Here!
use std::sync::atomic;
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    sync::atomic::AtomicBool,
};
use std::{marker::PhantomData, sync::Arc};

use append_only_vec::AppendOnlyVec;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_primitives::consensus::{BranchId, NetworkConstants, Parameters};
use zcash_primitives::zip339::Mnemonic;

use secp256k1::SecretKey;
use zcash_address::unified::{Container, Encoding, Typecode, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_encoding::{CompactSize, Vector};
use zcash_primitives::zip32::AccountId;
use zcash_primitives::{legacy::TransparentAddress, zip32::DiversifierIndex};
use zingoconfig::ZingoConfig;

use crate::wallet::traits::{DomainWalletExt, ReadableWriteable, Recipient};

use super::{
    extended_transparent::{ExtendedPrivKey, ExtendedPubKey, KeyIndex},
    get_zaddr_from_bip39seed, ToBase58Check,
};

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
        super::extended_transparent::ExtendedPubKey,
        super::extended_transparent::ExtendedPrivKey,
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
    pub fn ufvk(&self) -> Result<Ufvk, std::string::String> {
        use zcash_address::unified::Fvk as UfvkComponent;
        let o_fvk =
            UfvkComponent::Orchard(orchard::keys::FullViewingKey::try_from(self)?.to_bytes());
        let s_fvk = UfvkComponent::Sapling(
            sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(self)?.to_bytes(),
        );
        let mut t_fvk_bytes = [0u8; 65];
        let possible_transparent_key: Result<ExtendedPubKey, String> = self.try_into();
        if let Ok(t_ext_pk) = possible_transparent_key {
            t_fvk_bytes[0..32].copy_from_slice(&t_ext_pk.chain_code[..]);
            t_fvk_bytes[32..65].copy_from_slice(&t_ext_pk.public_key.serialize()[..]);
            let t_fvk = UfvkComponent::P2pkh(t_fvk_bytes);
            Ufvk::try_from_items(vec![o_fvk, s_fvk, t_fvk]).map_err(|e| e.to_string())
        } else {
            Ufvk::try_from_items(vec![o_fvk, s_fvk]).map_err(|e| e.to_string())
        }
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
        let previous_num_addresses = dbg!(self.addresses.len());
        let orchard_receiver = if desired_receivers.orchard {
            let fvk: orchard::keys::FullViewingKey = match self.try_into() {
                Ok(viewkey) => viewkey,
                Err(e) => {
                    self.addresses_write_lock
                        .swap(false, atomic::Ordering::Release);
                    return Err(e);
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
            let child_index = KeyIndex::from_index(self.addresses.len() as u32);
            let child_pk = match &self.transparent {
                Capability::Spend(ext_sk) => {
                    let secp = secp256k1::Secp256k1::new();
                    Some(
                        match ext_sk.derive_private_key(child_index) {
                            Err(e) => {
                                self.addresses_write_lock
                                    .swap(false, atomic::Ordering::Release);
                                return Err(format!(
                                    "Transparent private key derivation failed: {e}"
                                ));
                            }
                            Ok(res) => res.private_key,
                        }
                        .public_key(&secp),
                    )
                }
                Capability::View(ext_pk) => Some(match ext_pk.derive_public_key(child_index) {
                    Err(e) => {
                        self.addresses_write_lock
                            .swap(false, atomic::Ordering::Release);
                        return Err(format!("Transparent public key derivation failed: {e}"));
                    }
                    Ok(res) => res.public_key,
                }),
                Capability::None => None,
            };
            if let Some(pk) = child_pk {
                self.transparent_child_addresses.push((
                    self.addresses.len(),
                    #[allow(deprecated)]
                    zcash_primitives::legacy::keys::pubkey_to_address(&pk),
                ));
                Some(pk)
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
            transparent_receiver
                .as_ref()
                // This is deprecated. Not sure what the alternative is,
                // other than implementing it ourselves.
                .map(zcash_primitives::legacy::keys::pubkey_to_address),
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
        config: &ZingoConfig,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, String> {
        if let Capability::Spend(transparent_sk) = &self.transparent {
            self.transparent_child_addresses()
                .iter()
                .map(|(i, taddr)| -> Result<_, String> {
                    let hash = match taddr {
                        TransparentAddress::PublicKeyHash(hash) => hash,
                        TransparentAddress::ScriptHash(hash) => hash,
                    };
                    Ok((
                        hash.to_base58check(&config.chain.b58_pubkey_address_prefix(), &[]),
                        transparent_sk
                            .derive_private_key(KeyIndex::Normal(*i as u32))
                            .map_err(|e| e.to_string())?
                            .private_key,
                    ))
                })
                .collect::<Result<_, _>>()
        } else {
            Err("Wallet is no capable to spend transparent funds".to_string())
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_seed(config: &ZingoConfig, seed: &[u8; 64], position: u32) -> Self {
        let (sapling_key, _, _) = get_zaddr_from_bip39seed(config, seed, position);
        let transparent_parent_key =
            super::extended_transparent::ExtendedPrivKey::get_ext_taddr_from_bip39seed(
                config, seed, position,
            );

        let orchard_key = orchard::keys::SpendingKey::from_zip32_seed(
            seed,
            config.chain.coin_type(),
            AccountId::try_from(position).unwrap(),
        )
        .unwrap();
        Self {
            orchard: Capability::Spend(orchard_key),
            sapling: Capability::Spend(sapling_key),
            transparent: Capability::Spend(transparent_parent_key),
            ..Default::default()
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_phrase(
        config: &ZingoConfig,
        seed_phrase: &Mnemonic,
        position: u32,
    ) -> Result<Self, String> {
        // The seed bytes is the raw entropy. To pass it to HD wallet generation,
        // we need to get the 64 byte bip39 entropy
        let bip39_seed = seed_phrase.to_seed("");
        Ok(Self::new_from_seed(config, &bip39_seed, position))
    }

    /// Creates a new `WalletCapability` from a unified spending key.
    pub fn new_from_usk(usk: &[u8]) -> Result<Self, String> {
        // Decode unified spending key
        let usk = UnifiedSpendingKey::from_bytes(Era::Orchard, usk)
            .map_err(|_| "Error decoding unified spending key.")?;

        // Workaround https://github.com/zcash/librustzcash/issues/929 by serializing and deserializing the transparent key.
        let transparent_bytes = usk.transparent().to_bytes();
        let transparent_ext_key = transparent_key_from_bytes(transparent_bytes.as_slice())
            .map_err(|e| format!("Error processing transparent key: {}", e))?;

        Ok(Self {
            orchard: Capability::Spend(usk.orchard().to_owned()),
            sapling: Capability::Spend(usk.sapling().to_owned()),
            transparent: Capability::Spend(transparent_ext_key),
            ..Default::default()
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn new_from_ufvk(config: &ZingoConfig, ufvk_encoded: String) -> Result<Self, String> {
        // Decode UFVK
        if ufvk_encoded.starts_with(config.chain.hrp_sapling_extended_full_viewing_key()) {
            return Err("Viewing keys must be imported in the unified format".to_string());
        }
        let (network, ufvk) = Ufvk::decode(&ufvk_encoded)
            .map_err(|e| format!("Error decoding unified full viewing key: {}", e))?;
        if network != config.chain.network_type() {
            return Err("Given UFVK is not valid for current chain".to_string());
        }

        // Initialize an instance with no capabilities.
        let mut wc = WalletCapability::default();
        for fvk in ufvk.items() {
            use zcash_address::unified::Fvk as UfvkComponent;
            match fvk {
                UfvkComponent::Orchard(key_bytes) => {
                    wc.orchard = Capability::View(
                        orchard::keys::FullViewingKey::from_bytes(&key_bytes)
                            .ok_or("Orchard FVK deserialization failed")?,
                    );
                }
                UfvkComponent::Sapling(key_bytes) => {
                    wc.sapling = Capability::View(
                        sapling_crypto::zip32::DiversifiableFullViewingKey::read(
                            &key_bytes[..],
                            (),
                        )
                        .map_err(|e| e.to_string())?,
                    );
                }
                UfvkComponent::P2pkh(key_bytes) => {
                    wc.transparent = Capability::View(ExtendedPubKey {
                        chain_code: key_bytes[0..32].to_vec(),
                        public_key: secp256k1::PublicKey::from_slice(&key_bytes[32..65])
                            .map_err(|e| e.to_string())?,
                    });
                }
                UfvkComponent::Unknown { typecode, data: _ } => {
                    log::info!(
                        "Unknown receiver of type {} found in Unified Viewing Key",
                        typecode
                    );
                }
            }
        }
        Ok(wc)
    }

    pub(crate) fn get_all_taddrs(&self, config: &ZingoConfig) -> HashSet<String> {
        self.addresses
            .iter()
            .filter_map(|address| {
                address.transparent().and_then(|transparent_receiver| {
                    if let zcash_primitives::legacy::TransparentAddress::PublicKeyHash(hash) =
                        transparent_receiver
                    {
                        Some(super::ToBase58Check::to_base58check(
                            hash.as_slice(),
                            &config.chain.b58_pubkey_address_prefix(),
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

impl TryFrom<&WalletCapability> for super::extended_transparent::ExtendedPrivKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.transparent {
            Capability::Spend(sk) => Ok(sk.clone()),
            _ => Err("The wallet is not capable of spending transparent funds".to_string()),
        }
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.sapling {
            Capability::Spend(sk) => Ok(sk.clone()),
            _ => Err("The wallet is not capable of spending Sapling funds".to_string()),
        }
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::SpendingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.orchard {
            Capability::Spend(sk) => Ok(*sk),
            _ => Err("The wallet is not capable of spending Orchard funds".to_string()),
        }
    }
}

impl TryFrom<&WalletCapability> for super::extended_transparent::ExtendedPubKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.transparent {
            Capability::Spend(ext_sk) => Ok(ExtendedPubKey::from(ext_sk)),
            Capability::View(ext_pk) => Ok(ext_pk.clone()),
            Capability::None => {
                Err("The wallet is not capable of viewing transparent funds".to_string())
            }
        }
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::FullViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.orchard {
            Capability::Spend(sk) => Ok(orchard::keys::FullViewingKey::from(sk)),
            Capability::View(fvk) => Ok(fvk.clone()),
            Capability::None => {
                Err("The wallet is not capable of viewing Orchard funds".to_string())
            }
        }
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.sapling {
            Capability::Spend(sk) => {
                let dfvk = sk.to_diversifiable_full_viewing_key();
                Ok(dfvk)
            }
            Capability::View(fvk) => Ok(fvk.clone()),
            Capability::None => {
                Err("The wallet is not capable of viewing Sapling funds".to_string())
            }
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
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
        Ok(fvk.to_ovk(orchard::keys::Scope::External))
    }
}

impl TryFrom<&WalletCapability> for sapling_crypto::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
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

#[cfg(test)]
pub async fn get_transparent_secretkey_pubkey_taddr(
    lightclient: &crate::lightclient::LightClient,
) -> (
    Option<secp256k1::SecretKey>,
    Option<secp256k1::PublicKey>,
    Option<String>,
) {
    use super::address_from_pubkeyhash;

    let wc = lightclient.wallet.wallet_capability();
    // 2. Get an incoming transaction to a t address
    let (sk, pk) = match &wc.transparent {
        Capability::None => (None, None),
        Capability::View(ext_pk) => {
            let child_ext_pk = ext_pk.derive_public_key(KeyIndex::Normal(0)).ok();
            (None, child_ext_pk.map(|x| x.public_key))
        }
        Capability::Spend(master_sk) => {
            let secp = secp256k1::Secp256k1::new();
            let extsk = master_sk
                .derive_private_key(KeyIndex::Normal(wc.transparent_child_addresses[0].0 as u32))
                .unwrap();
            let pk = extsk.private_key.public_key(&secp);
            #[allow(deprecated)]
            (Some(extsk.private_key), Some(pk))
        }
    };
    let taddr = wc.addresses()[0]
        .transparent()
        .map(|taddr| address_from_pubkeyhash(&lightclient.config, *taddr));
    (sk, pk, taddr)
}
