use std::io;

use crate::{
    utils::{read_unified_address_from_raw_encoding, write_unified_address_to_raw_encoding},
    ParsedMemo,
};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::{CompactSize, Vector};

/// Packs a list of UAs into a memo. The UA only memo is version 0 of the protocol
/// Note that a UA's raw representation is 1 byte for length, +21 for a T-receiver,
/// +44 for a Sapling receiver, and +44 for an Orchard receiver. This totals a maximum
/// of 110 bytes per UA, and attempting to write more than 510 bytes will cause an error.
pub fn create_memo_v0(uas: &[UnifiedAddress]) -> io::Result<[u8; 511]> {
    let mut uas_bytes_vec = Vec::new();
    CompactSize::write(&mut uas_bytes_vec, 0usize)?;
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

/// Attempts to parse the 511 bytes of an arbitrary data memo
pub fn parse_memo(memo: [u8; 511]) -> io::Result<ParsedMemo> {
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
