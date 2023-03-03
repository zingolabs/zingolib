use std::io::{self, Read, Write};

use zcash_address::unified::{Address, Container, Encoding, Receiver};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::{CompactSize, Vector};
use zcash_primitives::consensus::BlockHeight;

/// A helper function to encode a UA as a CompactSize specifying the number
/// of receivers, followed by the UA's raw encoding as specified in
/// <https://zips.z.cash/zip-0316#encoding-of-unified-addresses>
pub fn write_unified_address_to_raw_encoding<W: Write>(
    ua: &UnifiedAddress,
    writer: W,
) -> io::Result<()> {
    let mainnet_encoded_ua = ua.encode(&zcash_primitives::consensus::MAIN_NETWORK);
    let (_mainnet, address) = Address::decode(&mainnet_encoded_ua).unwrap();
    let receivers = address.items();
    Vector::write(writer, &receivers, |mut w, receiver| {
        let (typecode, data): (u32, &[u8]) = match receiver {
            Receiver::Orchard(ref data) => (3, data),
            Receiver::Sapling(ref data) => (2, data),
            Receiver::P2pkh(ref data) => (0, data),
            Receiver::P2sh(ref data) => (1, data),
            Receiver::Unknown { typecode, ref data } => (*typecode, data.as_slice()),
        };
        CompactSize::write(&mut w, typecode as usize)?;
        CompactSize::write(&mut w, data.len())?;
        w.write_all(data)
    })
}

/// A helper function to decode a UA from a CompactSize specifying the number of
/// receivers, followed by the UA's raw encoding as specified in
/// <https://zips.z.cash/zip-0316#encoding-of-unified-addresses>
pub fn read_unified_address_from_raw_encoding<R: Read>(reader: R) -> io::Result<UnifiedAddress> {
    let receivers = Vector::read(reader, |mut r| {
        let typecode: usize = CompactSize::read_t(&mut r)?;
        let addr_len: usize = CompactSize::read_t(&mut r)?;
        let mut receiver_bytes = vec![0; addr_len];
        r.read_exact(&mut receiver_bytes)?;
        decode_receiver(typecode, receiver_bytes)
    })?;
    let address = Address::try_from_items(receivers)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    UnifiedAddress::try_from(address).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn decode_receiver(typecode: usize, data: Vec<u8>) -> io::Result<Receiver> {
    Ok(match typecode {
        0 => Receiver::P2pkh(<[u8; 20]>::try_from(data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Typecode {typecode} (P2pkh) indicates 20 bytes, found length of {}",
                    e.len()
                ),
            )
        })?),
        1 => Receiver::P2sh(<[u8; 20]>::try_from(data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Typecode {typecode} (P2sh) indicates 20 bytes, found length of {}",
                    e.len()
                ),
            )
        })?),
        2 => Receiver::Sapling(<[u8; 43]>::try_from(data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Typecode {typecode} (Sapling) indicates 43 bytes, found length of {}",
                    e.len()
                ),
            )
        })?),
        3 => Receiver::Orchard(<[u8; 43]>::try_from(data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Typecode {typecode} (Orchard) indicates 43 bytes, found length of {}",
                    e.len()
                ),
            )
        })?),
        _ => Receiver::Unknown {
            typecode: typecode as u32,
            data,
        },
    })
}

pub fn write_transaction_height_and_index<W: Write>(
    target_height: &BlockHeight,
    tx_height: &BlockHeight,
    index: usize,
    mut writer: W,
) -> io::Result<()> {
    if target_height < tx_height {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Attempted to write down a transaction in a higher block than previous transaction \
        or initial target height",
        ))
    } else {
        CompactSize::write(&mut writer, u32::from(*target_height - *tx_height) as usize)?;
        CompactSize::write(writer, index)
    }
}
