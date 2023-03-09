use std::io;

use crate::{
    utils::{
        read_transaction_relative_height_and_index, read_unified_address_from_raw_encoding,
        recover_absolute_transaction_heights, write_transaction_height_and_index,
        write_unified_address_to_raw_encoding,
    },
    ParsedMemo,
};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::{CompactSize, Vector};
use zcash_primitives::consensus::BlockHeight;

/// Packs a list of UAs into a memo. The UA only memo is version 0 of the protocol
/// Note that a UA's raw representation is 1 byte for length, +21 for a T-receiver,
/// +44 for a Sapling receiver, and +44 for an Orchard receiver. This totals a maximum
/// of 110 bytes per UA, and attempting to write more than 510 bytes will cause an error.
pub fn create_memo_v0(uas: impl AsRef<[UnifiedAddress]>) -> io::Result<[u8; 511]> {
    let mut uas_bytes_vec = Vec::new();
    CompactSize::write(&mut uas_bytes_vec, 0usize)?;
    Vector::write(&mut uas_bytes_vec, uas.as_ref(), |mut w, ua| {
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

pub fn create_memo_v1(
    uas: impl AsRef<[UnifiedAddress]>,
    mut target_height: BlockHeight,
    transaction_heights_and_indexes: impl AsRef<[(BlockHeight, usize)]>,
) -> io::Result<[u8; 511]> {
    let mut memo_bytes_vec = Vec::new();
    CompactSize::write(&mut memo_bytes_vec, 1usize)?;
    Vector::write(&mut memo_bytes_vec, uas.as_ref(), |mut w, ua| {
        write_unified_address_to_raw_encoding(&ua, &mut w)
    })?;
    let heights_indexes_and_target_heights = transaction_heights_and_indexes.as_ref().iter().fold(
        Vec::new(),
        |mut acc, (height, index)| {
            acc.push((*height, *index, target_height));
            target_height = *height;
            acc
        },
    );
    Vector::write(
        &mut memo_bytes_vec,
        &heights_indexes_and_target_heights,
        |mut w, (height, index, target_height)| {
            let result = write_transaction_height_and_index(&target_height, height, *index, &mut w);
            result
        },
    )?;
    let mut memo_bytes = [0u8; 511];
    if memo_bytes_vec.len() > 511 {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Too much to fit in memo field",
        ))
    } else {
        memo_bytes[..memo_bytes_vec.len()].copy_from_slice(memo_bytes_vec.as_slice());
        Ok(memo_bytes)
    }
}

/// Attempts to parse the 511 bytes of an arbitrary data memo
pub fn parse_memo(memo: [u8; 511], height: BlockHeight) -> io::Result<ParsedMemo> {
    let mut reader: &[u8] = &memo;
    match CompactSize::read(&mut reader)? {
        0 => Ok(ParsedMemo::Version0 {
            uas: Vector::read(&mut reader, |mut r| {
                read_unified_address_from_raw_encoding(&mut r)
            })?,
        }),
        1 => {
            let uas = Vector::read(&mut reader, |mut r| {
                read_unified_address_from_raw_encoding(&mut r)
            })?;
            let transaction_relative_heights_and_indexes = Vector::read(&mut reader, |r| {
                read_transaction_relative_height_and_index(r)
            })?;
            let transaction_heights_and_indexes = recover_absolute_transaction_heights(
                height,
                transaction_relative_heights_and_indexes,
            )?;
            Ok(ParsedMemo::Version1 {
                uas,
                transaction_heights_and_indexes,
            })
        }
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

    #[test]
    fn ser_deser_v1_memo() {
        use std::iter::repeat;
        let uas = Vec::new();
        let height = BlockHeight::from(1000);
        let transaction_heights_and_indexes =
            [997, 997, 993, 991, 931, 757, 757, 700, 666, 665, 200]
                .into_iter()
                .map(BlockHeight::from)
                .zip(repeat(0))
                .collect();

        let memo_bytes =
            create_memo_v1(uas.clone(), height, &transaction_heights_and_indexes).unwrap();
        assert_eq!(
            ParsedMemo::Version1 {
                uas,
                transaction_heights_and_indexes
            },
            parse_memo(memo_bytes, height).unwrap()
        )
    }
}
