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

pub(crate) fn write_transaction_height_and_index<W: Write>(
    last_noted_height: &Option<BlockHeight>,
    tx_height: &BlockHeight,
    index: usize,
    mut writer: W,
) -> io::Result<()> {
    if let Some(last_noted_height) = last_noted_height {
        if last_noted_height < tx_height {
            Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Attempted to write down a transaction in a higher block than previous transaction \
        or initial target height",
        ))
        } else {
            CompactSize::write(
                &mut writer,
                u32::from(*last_noted_height - *tx_height) as usize,
            )
        }
    } else {
        CompactSize::write(&mut writer, u32::from(*tx_height) as usize)
    }?;
    CompactSize::write(writer, index)
}
pub(crate) fn read_transaction_relative_height_and_index<R: Read>(
    mut reader: R,
) -> io::Result<(BlockHeight, usize)> {
    let relative_height = BlockHeight::from_u32(CompactSize::read_t(&mut reader)?);
    let index = CompactSize::read_t(&mut reader)?;
    Ok((relative_height, index))
}

pub(crate) fn recover_absolute_transaction_heights(
    // The initial value is the height of the transaction that the memo was encoded in
    relative_heights_and_indexes: Vec<(BlockHeight, usize)>,
) -> io::Result<Vec<(BlockHeight, usize)>> {
    let mut last_noted_height = None;
    relative_heights_and_indexes
        .iter()
        .try_fold(Vec::new(), |mut acc, (height, index)| {
            let absolute_height =
                BlockHeight::from_u32(if let Some(last_noted_height) = last_noted_height {
                    u32::from(last_noted_height)
                        .checked_sub(u32::from(*height))
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Found transaction at height below 0",
                            )
                        })?
                } else {
                    u32::from(*height)
                });
            acc.push((absolute_height, *index));
            last_noted_height = Some(absolute_height);
            Ok(acc)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::repeat;
    #[test]
    fn recover_correct_transaction_heights_including_relative_zeroes() -> io::Result<()> {
        let relative_heights_and_indexes = [90, 5, 0, 2, 0, 3]
            .into_iter()
            .map(BlockHeight::from)
            .zip(repeat(0))
            .collect();
        assert_eq!(
            recover_absolute_transaction_heights(relative_heights_and_indexes)?,
            [90, 85, 85, 83, 83, 80]
                .into_iter()
                .map(BlockHeight::from)
                .zip(repeat(0))
                .collect::<Vec<(BlockHeight, usize)>>()
        );
        Ok(())
    }
}
