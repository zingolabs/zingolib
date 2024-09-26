use std::io::{self, Read, Write};

use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::LittleEndian;
use zcash_address::unified::Typecode;
use zcash_encoding::CompactSize;
use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::legacy::keys::AccountPrivKey;

use crate::wallet::{traits::ReadableWriteable, LightWallet};

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

pub(crate) fn legacy_fvks_to_ufvk<P: zcash_primitives::consensus::Parameters>(
    orchard_fvk: Option<orchard::keys::FullViewingKey>,
    sapling_fvk: Option<sapling_crypto::zip32::DiversifiableFullViewingKey>,
    transparent_fvk: Option<extended_transparent::ExtendedPubKey>,
    parameters: &P,
) -> Result<UnifiedFullViewingKey, std::string::String> {
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

    let ufvk = zcash_address::unified::Ufvk::try_from_items(fvks).map_err(|e| e.to_string())?;

    UnifiedFullViewingKey::decode(parameterss, &ufvk.encode(&parameterss.network_type()))
}
