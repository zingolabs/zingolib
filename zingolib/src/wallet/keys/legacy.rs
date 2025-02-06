//! Module for legacy code associated with wallet keys required for backward-compatility with old wallet versions

use std::io::{self, Read, Write};

use bip32::ExtendedPublicKey;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use zcash_address::unified::Typecode;
use zcash_encoding::CompactSize;
use zcash_keys::keys::{Era, UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::legacy::{
    keys::{AccountPubKey, NonHardenedChildIndex},
    TransparentAddress,
};

use crate::wallet::{error::KeyError, traits::ReadableWriteable};

use super::unified::{KEY_TYPE_EMPTY, KEY_TYPE_SPEND, KEY_TYPE_VIEW};

pub mod extended_transparent;

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
