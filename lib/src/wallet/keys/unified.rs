use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
};

use bip0039::Mnemonic;
use byteorder::{ReadBytesExt, WriteBytesExt};
use orchard::keys::Scope;

use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::Vector;
use zcash_primitives::{legacy::TransparentAddress, zip32::DiversifierIndex};
use zingoconfig::ZingoConfig;

use crate::wallet::traits::ReadableWriteable;

use super::{extended_transparent::KeyIndex, Keys, ToBase58Check};

#[derive(Clone, Debug)]
pub struct UnifiedSpendAuthority {
    orchard_key: orchard::keys::SpendingKey,
    sapling_key: zcash_primitives::zip32::ExtendedSpendingKey,
    transparent_parent_key: super::extended_transparent::ExtendedPrivKey,
    transparent_child_keys: Vec<(usize, secp256k1::SecretKey)>,

    addresses: Vec<UnifiedAddress>,
    // Not all diversifier indexes produce valid sapling addresses.
    // Because of this, the index isn't necessarily equal to addresses.len()
    next_sapling_diversifier_index: DiversifierIndex,

    // Note that unified spend authority encryption is not yet implemented,
    // These are placeholder fields, and are currenly always false
    pub(crate) encrypted: bool,
    pub(crate) unlocked: bool,
}

#[derive(Debug, Clone, Copy)]
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
        let recievers = reader.read_u8()?;
        Ok(Self {
            orchard: recievers & 0b1 != 0,
            sapling: recievers & 0b10 != 0,
            transparent: recievers & 0b100 != 0,
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
        .into_iter()
        .map(|n| ReceiverSelection::read([1, n].as_slice(), ()).unwrap())
        .enumerate()
    {
        let mut recievers_selected_bytes = [0; 2];
        receivers_selected
            .write(recievers_selected_bytes.as_mut_slice())
            .unwrap();
        assert_eq!(i as u8, recievers_selected_bytes[1]);
    }
}

impl UnifiedSpendAuthority {
    pub fn addresses(&self) -> &[UnifiedAddress] {
        &self.addresses
    }

    pub fn transparent_child_keys(&self) -> &Vec<(usize, secp256k1::SecretKey)> {
        &self.transparent_child_keys
    }

    pub fn new_address(
        &mut self,
        desired_receivers: ReceiverSelection,
    ) -> Result<UnifiedAddress, String> {
        let orchard_receiver = if desired_receivers.orchard {
            Some(
                orchard::keys::FullViewingKey::from(&self.orchard_key)
                    .address_at(self.addresses.len(), Scope::External),
            )
        } else {
            None
        };
        let sapling_reciever = if desired_receivers.sapling {
            let (mut new_index, address) =
                zcash_primitives::zip32::ExtendedFullViewingKey::from(&self.sapling_key)
                    .find_address(self.next_sapling_diversifier_index)
                    .expect("Diversifier index overflow");
            new_index.increment().expect("Diversifier index overflow");
            self.next_sapling_diversifier_index = new_index;
            Some(address)
        } else {
            None
        };
        let transparent_receiver = if desired_receivers.transparent {
            let key_index =
                KeyIndex::hardened_from_normalize_index(self.addresses.len() as u32).unwrap();
            let new_key = self
                .transparent_parent_key
                .derive_private_key(key_index)
                .expect("Private key derevation failed")
                .private_key;
            let secp = secp256k1::Secp256k1::new();
            let address = secp256k1::PublicKey::from_secret_key(&secp, &new_key);
            self.transparent_child_keys
                .push((self.addresses.len(), new_key));
            Some(address)
        } else {
            None
        };
        let ua = UnifiedAddress::from_receivers(
            orchard_receiver,
            sapling_reciever,
            #[allow(deprecated)]
            transparent_receiver
                .as_ref()
                // This is deprecated. Not sure what the alternative is,
                // other than implementing it ourselves.
                .map(zcash_primitives::legacy::keys::pubkey_to_address),
        )
        .ok_or(format!(
            "Invalid receivers requested! At least one of sapling or orchard required"
        ))?;
        self.addresses.push(ua.clone());
        Ok(ua)
    }

    pub fn get_taddr_to_sk_map(
        &self,
        config: &ZingoConfig,
    ) -> HashMap<String, secp256k1::SecretKey> {
        self.addresses
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
                    hash.to_base58check(&config.base58_script_address(), &[]),
                    key.1.clone(),
                )
            })
            .collect()
    }
    pub fn new_from_seed(config: &ZingoConfig, seed: &[u8; 64], position: u32) -> Self {
        let (sapling_key, _, _) = Keys::get_zaddr_from_bip39seed(config, seed, position);
        let transparent_parent_key =
            super::extended_transparent::ExtendedPrivKey::get_ext_taddr_from_bip39seed(
                config, seed, position,
            );

        let orchard_key =
            orchard::keys::SpendingKey::from_zip32_seed(seed, config.get_coin_type(), position)
                .unwrap();
        Self {
            sapling_key,
            orchard_key,
            transparent_parent_key,
            transparent_child_keys: vec![],
            addresses: vec![],
            next_sapling_diversifier_index: DiversifierIndex::new(),
            encrypted: false,
            unlocked: true,
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

    pub(crate) fn get_all_taddrs(&self, config: &ZingoConfig) -> HashSet<String> {
        self.addresses
            .iter()
            .filter_map(|address| {
                address.transparent().and_then(|transparent_reciever| {
                    if let zcash_primitives::legacy::TransparentAddress::PublicKey(hash) =
                        transparent_reciever
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

    pub fn encrypt(&mut self, _passwd: String) -> std::io::Result<()> {
        todo!()
    }
    pub fn lock(&mut self) -> std::io::Result<()> {
        todo!()
    }
    pub fn unlock(&mut self, _passwd: String) -> std::io::Result<()> {
        todo!()
    }
    pub fn remove_encryption(&mut self, _passwd: String) -> std::io::Result<()> {
        todo!()
    }
}
impl ReadableWriteable<()> for UnifiedSpendAuthority {
    const VERSION: u8 = 1;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let mut orchard_key_bytes = [0; 32];
        reader.read_exact(&mut orchard_key_bytes)?;
        let orchard_key: orchard::keys::SpendingKey =
            Option::from(orchard::keys::SpendingKey::from_bytes(orchard_key_bytes)).ok_or(
                io::Error::new(io::ErrorKind::InvalidData, "Invalid orchard spending key"),
            )?;
        let sapling_key = zcash_primitives::zip32::ExtendedSpendingKey::read(&mut reader)?;
        let transparent_parent_key =
            super::extended_transparent::ExtendedPrivKey::read(&mut reader, ())?;
        let receivers_selected =
            Vector::read(&mut reader, |mut r| ReceiverSelection::read(&mut r, ()))?;
        let encrypted = match reader.read_u8()? {
            0 => false,
            1 => true,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "encrypted status is not bool!",
                ))
            }
        };
        let mut usa = Self {
            orchard_key,
            sapling_key,
            transparent_parent_key,
            transparent_child_keys: vec![],
            addresses: vec![],
            next_sapling_diversifier_index: DiversifierIndex::new(),
            encrypted,
            unlocked: true,
        };
        for receiver_selection in receivers_selected {
            usa.new_address(receiver_selection)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        }

        Ok(usa)
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        writer.write(self.orchard_key.to_bytes())?;
        self.sapling_key.write(&mut writer)?;
        self.transparent_parent_key.write(&mut writer)?;
        let mut receivers_per_address = Vec::new();
        for address in &self.addresses {
            receivers_per_address.push(ReceiverSelection {
                orchard: address.orchard().is_some(),
                sapling: address.sapling().is_some(),
                transparent: address.transparent().is_some(),
            })
        }
        Vector::write(
            &mut writer,
            &receivers_per_address,
            |mut w, receiver_selection| receiver_selection.write(&mut w),
        )
    }
}

impl From<&UnifiedSpendAuthority> for zcash_primitives::zip32::ExtendedSpendingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        usa.sapling_key.clone()
    }
}

impl From<&UnifiedSpendAuthority> for orchard::keys::SpendingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        usa.orchard_key.clone()
    }
}

impl From<&UnifiedSpendAuthority> for orchard::keys::IncomingViewingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        orchard::keys::FullViewingKey::from(&usa.orchard_key).to_ivk(Scope::External)
    }
}

impl From<&UnifiedSpendAuthority> for zcash_primitives::sapling::SaplingIvk {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        zcash_primitives::zip32::ExtendedFullViewingKey::from(&usa.sapling_key)
            .fvk
            .vk
            .ivk()
    }
}
impl From<&UnifiedSpendAuthority> for orchard::keys::FullViewingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        orchard::keys::FullViewingKey::from(&usa.orchard_key)
    }
}

impl From<&UnifiedSpendAuthority> for zcash_primitives::zip32::ExtendedFullViewingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        zcash_primitives::zip32::ExtendedFullViewingKey::from(&usa.sapling_key)
    }
}
impl From<&UnifiedSpendAuthority> for orchard::keys::OutgoingViewingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        orchard::keys::FullViewingKey::from(&usa.orchard_key).to_ovk(Scope::External)
    }
}

impl From<&UnifiedSpendAuthority> for zcash_primitives::keys::OutgoingViewingKey {
    fn from(usa: &UnifiedSpendAuthority) -> Self {
        zcash_primitives::zip32::ExtendedFullViewingKey::from(&usa.sapling_key)
            .fvk
            .ovk
    }
}
