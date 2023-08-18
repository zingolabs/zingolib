use std::sync::atomic;
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    sync::atomic::AtomicBool,
};

use append_only_vec::AppendOnlyVec;
use bip0039::Mnemonic;
use byteorder::{ReadBytesExt, WriteBytesExt};
use orchard::keys::Scope;

use zcash_address::unified::{Container, Encoding, Fvk, Ufvk};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::Vector;
use zcash_primitives::{
    legacy::TransparentAddress, sapling::note_encryption::PreparedIncomingViewingKey,
    zip32::DiversifierIndex,
};
use zingoconfig::ZingoConfig;

use crate::wallet::traits::ReadableWriteable;

use super::{
    extended_transparent::{ExtendedPubKey, KeyIndex},
    get_zaddr_from_bip39seed, ToBase58Check,
};

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Capability<ViewingKeyType, SpendKeyType> {
    None,
    View(ViewingKeyType),
    Spend(SpendKeyType),
}

impl<V, S> Capability<V, S> {
    pub fn can_spend(&self) -> bool {
        matches!(self, Capability::Spend(_))
    }

    pub fn can_view(&self) -> bool {
        match self {
            Capability::None => false,
            Capability::View(_) => true,
            Capability::Spend(_) => true,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Capability::None => "No key",
            Capability::View(_) => "View only",
            Capability::Spend(_) => "Spend capable",
        }
    }
}

#[derive(Debug)]
pub struct WalletCapability {
    pub transparent: Capability<
        super::extended_transparent::ExtendedPubKey,
        super::extended_transparent::ExtendedPrivKey,
    >,
    pub sapling: Capability<
        zcash_primitives::zip32::sapling::DiversifiableFullViewingKey,
        zcash_primitives::zip32::sapling::ExtendedSpendingKey,
    >,
    pub orchard: Capability<orchard::keys::FullViewingKey, orchard::keys::SpendingKey>,

    transparent_child_keys: append_only_vec::AppendOnlyVec<(usize, secp256k1::SecretKey)>,
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
            transparent_child_keys: AppendOnlyVec::new(),
            addresses: AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ReceiverSelection {
    pub orchard: bool,
    pub sapling: bool,
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
    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        &self.addresses
    }

    pub fn transparent_child_keys(
        &self,
    ) -> Result<&AppendOnlyVec<(usize, secp256k1::SecretKey)>, String> {
        if self.transparent.can_spend() {
            Ok(&self.transparent_child_keys)
        } else {
            Err("The wallet is not capable of spending transparent funds.".to_string())
        }
    }

    pub(crate) fn ufvk(&self) -> Result<Ufvk, zcash_address::unified::ParseError> {
        let o_fvk = Fvk::Orchard(
            orchard::keys::FullViewingKey::try_from(self)
                .unwrap()
                .to_bytes(),
        );
        let s_fvk = Fvk::Sapling(
            zcash_primitives::zip32::sapling::DiversifiableFullViewingKey::try_from(self)
                .unwrap()
                .to_bytes(),
        );
        let mut t_fvk_bytes = [0u8; 65];
        let t_ext_pk: ExtendedPubKey = self.try_into().unwrap();
        t_fvk_bytes[0..32].copy_from_slice(&t_ext_pk.chain_code[..]);
        t_fvk_bytes[32..65].copy_from_slice(&t_ext_pk.public_key.serialize()[..]);
        let t_fvk = Fvk::P2pkh(t_fvk_bytes);
        use zcash_address::unified::Encoding as _;
        Ufvk::try_from_items(vec![o_fvk, s_fvk, t_fvk])
    }

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
                    return Err(e);
                }
            };
            Some(fvk.address_at(self.addresses.len(), Scope::External))
        } else {
            None
        };

        // produce a Sapling address to increment Sapling diversifier index
        let sapling_receiver = if desired_receivers.sapling && self.sapling.can_view() {
            let mut sapling_diversifier_index = DiversifierIndex::new();
            let mut address;
            let mut count = 0;
            let fvk: zcash_primitives::zip32::sapling::DiversifiableFullViewingKey =
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
            let child_index = KeyIndex::from_index(self.addresses.len() as u32).unwrap();
            match &self.transparent {
                Capability::Spend(ext_sk) => {
                    let child_sk = match ext_sk.derive_private_key(child_index) {
                        Err(e) => {
                            self.addresses_write_lock
                                .swap(false, atomic::Ordering::Release);
                            return Err(format!("Transparent private key derivation failed: {e}"));
                        }
                        Ok(res) => res.private_key,
                    };
                    let secp = secp256k1::Secp256k1::new();
                    let child_pk = secp256k1::PublicKey::from_secret_key(&secp, &child_sk);
                    self.transparent_child_keys
                        .push((self.addresses.len(), child_sk));
                    Some(child_pk)
                }
                Capability::View(ext_pk) => {
                    let child_pk = match ext_pk.derive_public_key(child_index) {
                        Err(e) => {
                            self.addresses_write_lock
                                .swap(false, atomic::Ordering::Release);
                            return Err(format!("Transparent public key derivation failed: {e}"));
                        }
                        Ok(res) => res.public_key,
                    };
                    Some(child_pk)
                }
                Capability::None => None,
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

    pub fn get_taddr_to_secretkey_map(
        &self,
        config: &ZingoConfig,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, String> {
        if self.transparent.can_spend() {
            Ok(self
                .addresses
                .iter()
                .enumerate()
                .filter_map(|(i, ua)| {
                    ua.transparent().zip(
                        self.transparent_child_keys
                            .iter()
                            .find(|(index, _key)| i == *index),
                    )
                })
                .map(|(taddr, key)| {
                    let hash = match taddr {
                        TransparentAddress::PublicKey(hash) => hash,
                        TransparentAddress::Script(hash) => hash,
                    };
                    (
                        hash.to_base58check(&config.base58_pubkey_address(), &[]),
                        key.1,
                    )
                })
                .collect())
        } else {
            Err("Wallet is no capable to spend transparent funds".to_string())
        }
    }

    pub fn new_from_seed(config: &ZingoConfig, seed: &[u8; 64], position: u32) -> Self {
        let (sapling_key, _, _) = get_zaddr_from_bip39seed(config, seed, position);
        let transparent_parent_key =
            super::extended_transparent::ExtendedPrivKey::get_ext_taddr_from_bip39seed(
                config, seed, position,
            );

        let orchard_key =
            orchard::keys::SpendingKey::from_zip32_seed(seed, config.get_coin_type(), position)
                .unwrap();
        Self {
            orchard: Capability::Spend(orchard_key),
            sapling: Capability::Spend(sapling_key),
            transparent: Capability::Spend(transparent_parent_key),
            ..Default::default()
        }
    }

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

    pub fn new_from_ufvk(config: &ZingoConfig, ufvk_encoded: String) -> Result<Self, String> {
        // Decode UFVK
        if ufvk_encoded.starts_with(config.hrp_sapling_viewing_key()) {
            return Err("Viewing keys must be imported in the unified format".to_string());
        }
        let (network, ufvk) = Ufvk::decode(&ufvk_encoded)
            .map_err(|e| format!("Error decoding unified full viewing key: {}", e))?;
        if network != config.chain.to_zcash_address_network() {
            return Err("Given UFVK is not valid for current chain".to_string());
        }

        // Initialize an instance with no capabilities.
        let mut wc = WalletCapability::default();
        for fvk in ufvk.items() {
            match fvk {
                Fvk::Orchard(key_bytes) => {
                    wc.orchard = Capability::View(
                        orchard::keys::FullViewingKey::from_bytes(&key_bytes)
                            .ok_or("Orchard FVK deserialization failed")?,
                    );
                }
                Fvk::Sapling(key_bytes) => {
                    wc.sapling = Capability::View(
                        zcash_primitives::zip32::sapling::DiversifiableFullViewingKey::read(
                            &key_bytes[..],
                            (),
                        )
                        .map_err(|e| e.to_string())?,
                    );
                }
                Fvk::P2pkh(key_bytes) => {
                    wc.transparent = Capability::View(ExtendedPubKey {
                        chain_code: key_bytes[0..32].to_vec(),
                        public_key: secp256k1::PublicKey::from_slice(&key_bytes[32..65])
                            .map_err(|e| e.to_string())?,
                    });
                }
                Fvk::Unknown { typecode, data: _ } => {
                    log::info!(
                        "Unknown receiver of type {} found in Unified Viewing Key",
                        typecode
                    );
                }
            }
        }
        Ok(wc)
    }

    pub(crate) fn get_ua_from_contained_transparent_receiver(
        &self,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        self.addresses
            .iter()
            .find(|ua| ua.transparent() == Some(receiver))
            .cloned()
    }
    pub(crate) fn get_all_taddrs(&self, config: &ZingoConfig) -> HashSet<String> {
        self.addresses
            .iter()
            .filter_map(|address| {
                address.transparent().and_then(|transparent_receiver| {
                    if let zcash_primitives::legacy::TransparentAddress::PublicKey(hash) =
                        transparent_receiver
                    {
                        Some(super::ToBase58Check::to_base58check(
                            hash.as_slice(),
                            &config.base58_pubkey_address(),
                            &[],
                        ))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    pub fn first_sapling_address(&self) -> &zcash_primitives::sapling::PaymentAddress {
        // This index is dangerous, but all ways to instanciate a UnifiedSpendAuthority
        // create it with a suitable first address
        self.addresses()[0].sapling().unwrap()
    }

    /// Returns a selection of pools where the wallet can spend funds.
    pub fn can_spend_from_all_pools(&self) -> bool {
        self.orchard.can_spend() && self.sapling.can_spend() && self.transparent.can_spend()
    }

    //TODO: NAME?????!!
    pub fn get_trees_witness_trees(&self) -> Option<crate::wallet::data::WitnessTrees> {
        if self.can_spend_from_all_pools() {
            Some(crate::wallet::data::WitnessTrees::default())
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
                    format!("Unknow wallet Capability type: {}", x),
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
                let sapling = zcash_primitives::zip32::ExtendedSpendingKey::read(&mut reader)?;
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

impl TryFrom<&WalletCapability> for super::extended_transparent::ExtendedPrivKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        match &wc.transparent {
            Capability::Spend(sk) => Ok(sk.clone()),
            _ => Err("The wallet is not capable of spending transparent funds".to_string()),
        }
    }
}

impl TryFrom<&WalletCapability> for zcash_primitives::zip32::ExtendedSpendingKey {
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

impl TryFrom<&WalletCapability> for zcash_primitives::zip32::sapling::DiversifiableFullViewingKey {
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

impl TryFrom<&WalletCapability> for PreparedIncomingViewingKey {
    type Error = String;

    fn try_from(value: &WalletCapability) -> Result<Self, Self::Error> {
        zcash_primitives::sapling::SaplingIvk::try_from(value)
            .map(|k| PreparedIncomingViewingKey::new(&k))
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::IncomingViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
        Ok(fvk.to_ivk(Scope::External))
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::PreparedIncomingViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        orchard::keys::IncomingViewingKey::try_from(wc)
            .map(|k| orchard::keys::PreparedIncomingViewingKey::new(&k))
    }
}

impl TryFrom<&WalletCapability> for zcash_primitives::sapling::SaplingIvk {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        let fvk: zcash_primitives::zip32::sapling::DiversifiableFullViewingKey = wc.try_into()?;
        Ok(fvk.fvk().vk.ivk())
    }
}

impl TryFrom<&WalletCapability> for orchard::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
        Ok(fvk.to_ovk(Scope::External))
    }
}

impl TryFrom<&WalletCapability> for zcash_primitives::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(wc: &WalletCapability) -> Result<Self, String> {
        let fvk: zcash_primitives::zip32::sapling::DiversifiableFullViewingKey = wc.try_into()?;
        Ok(fvk.fvk().ovk)
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
        Capability::Spend(_) => {
            let sk = wc.transparent_child_keys[0].1;
            let secp = secp256k1::Secp256k1::new();
            let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);
            (Some(sk), Some(pk))
        }
    };
    let taddr = wc.addresses()[0]
        .transparent()
        .map(|taddr| address_from_pubkeyhash(&lightclient.config, *taddr));
    (sk, pk, taddr)
}
