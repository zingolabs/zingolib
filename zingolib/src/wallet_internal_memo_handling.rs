use std::io::{self, Read, Write};

use zcash_address::unified::{Address, Container, Encoding, Receiver};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::{CompactSize, Vector};

pub const MEMO_VERSION: usize = 0;

#[non_exhaustive]
pub enum ParsedMemo {
    Version0 { uas: Vec<UnifiedAddress> },
}

pub fn create_wallet_internal_memo_version_0(uas: &[UnifiedAddress]) -> io::Result<[u8; 511]> {
    let mut uas_bytes_vec = Vec::new();
    CompactSize::write(&mut uas_bytes_vec, MEMO_VERSION)?;
    Vector::write(&mut uas_bytes_vec, uas, |mut w, ua| {
        write_unified_address_to_raw_encoding(&ua, &mut w)
    })?;
    let mut uas_bytes = [0u8; 511];
    if uas_bytes_vec.len() > 511 {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Too many uas to fit in memo field",
        ))
    } else {
        uas_bytes[..uas_bytes_vec.len()].copy_from_slice(uas_bytes_vec.as_slice());
        Ok(uas_bytes)
    }
}

pub fn read_wallet_internal_memo(memo: [u8; 511]) -> io::Result<ParsedMemo> {
    let mut reader: &[u8] = &memo;
    match CompactSize::read(&mut reader)? {
        0 => Ok(ParsedMemo::Version0 {
            uas: Vector::read(&mut reader, |mut r| {
                read_unified_address_from_raw_encoding(&mut r)
            })?,
        }),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Received memo from a future version of this protocol.\n\
            Please ensure your software is up-to-date",
            ))
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_vectors::UA_TEST_VECTORS;
    use zcash_client_backend::address::RecipientAddress;
    use zcash_primitives::consensus::MAIN_NETWORK;

    #[test]
    fn round_trip_ser_deser() {
        for test_vector in UA_TEST_VECTORS {
            let RecipientAddress::Unified(ua) =
                RecipientAddress::decode(&MAIN_NETWORK, test_vector.unified_addr).unwrap()
            else {
                panic!("Couldn't decode test_vector UA")
            };
            let mut serialized_ua = Vec::new();
            write_unified_address_to_raw_encoding(&ua, &mut serialized_ua).unwrap();
            assert_eq!(
                ua,
                read_unified_address_from_raw_encoding(&*serialized_ua).unwrap()
            );
        }
    }
}
