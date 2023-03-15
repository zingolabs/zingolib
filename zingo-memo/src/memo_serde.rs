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

fn write_to_vec<T: AsRef<[UnifiedAddress]>>(uas: T, version: usize) -> io::Result<Vec<u8>> {
    let mut uas_bytes_vec = Vec::new();
    CompactSize::write(&mut uas_bytes_vec, version)?;
    Vector::write(&mut uas_bytes_vec, uas.as_ref(), |mut w, ua| {
        write_unified_address_to_raw_encoding(&ua, &mut w)
    })?;
    Ok(uas_bytes_vec)
}
fn packed_to_array(packed: Vec<u8>) -> io::Result<[u8; 511]> {
    let mut uas_bytes = [0u8; 511];
    if packed.len() > 511 {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Too many uas to fit in memo field",
        ))
    } else {
        uas_bytes[..packed.len()].copy_from_slice(packed.as_slice());
        Ok(uas_bytes)
    }
}
fn add_txdata(
    memo_bytes_vec: &mut Vec<u8>,
    mut transaction_heights_and_indices: Vec<(BlockHeight, usize)>,
) -> io::Result<()> {
    let mut last_noted_height = None;
    transaction_heights_and_indices.sort_unstable();
    transaction_heights_and_indices.reverse();
    let heights_indexes_and_target_heights =
        transaction_heights_and_indices
            .iter()
            .fold(Vec::new(), |mut acc, (height, index)| {
                acc.push((*height, *index, last_noted_height));
                last_noted_height = Some(*height);
                acc
            });
    Vector::write(
        memo_bytes_vec,
        &heights_indexes_and_target_heights,
        |mut w, (height, index, last_noted_height)| {
            let result =
                write_transaction_height_and_index(last_noted_height, height, *index, &mut w);
            result
        },
    )?;
    Ok(())
}
/// Packs a list of UAs into a memo. The UA only memo is version 0 of the protocol
/// Note that a UA's raw representation is 1 byte for length, +21 for a T-receiver,
/// +44 for a Sapling receiver, and +44 for an Orchard receiver. This totals a maximum
/// of 110 bytes per UA, and attempting to write more than 510 bytes will cause an error.
pub fn create_memo_v0<T: AsRef<[UnifiedAddress]>>(uas: T) -> io::Result<[u8; 511]> {
    let memo_bytes_vec = write_to_vec(uas, 0usize)?;
    packed_to_array(memo_bytes_vec)
}

pub fn create_memo_v1<T: AsRef<[UnifiedAddress]>>(
    uas: T,
    transaction_heights_and_indices: Vec<(BlockHeight, usize)>,
) -> io::Result<[u8; 511]> {
    let mut memo_bytes_vec = write_to_vec(uas, 1usize)?;
    add_txdata(&mut memo_bytes_vec, transaction_heights_and_indices)?;
    packed_to_array(memo_bytes_vec)
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
        1 => {
            let uas = Vector::read(&mut reader, |mut r| {
                read_unified_address_from_raw_encoding(&mut r)
            })?;
            let transaction_relative_heights_and_indexes = Vector::read(&mut reader, |r| {
                read_transaction_relative_height_and_index(r)
            })?;
            let transaction_heights_and_indexes =
                recover_absolute_transaction_heights(transaction_relative_heights_and_indexes)?;
            Ok(ParsedMemo::Version1 {
                uas,
                transaction_heights_and_indexes,
            })
        }
        x @ _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Received memo from '{}', a future version of this protocol.\n\
            Please ensure your software is up-to-date",
                    x
                ),
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
        let transaction_heights_and_indexes: Vec<_> =
            [997, 997, 993, 991, 931, 757, 757, 700, 666, 665, 200]
                .into_iter()
                .map(BlockHeight::from)
                .zip(repeat(0))
                .collect();

        let memo_bytes =
            create_memo_v1(uas.clone(), transaction_heights_and_indexes.clone()).unwrap();
        assert_eq!(
            ParsedMemo::Version1 {
                uas,
                transaction_heights_and_indexes
            },
            parse_memo(memo_bytes).unwrap()
        )
    }
    #[test]
    fn parse_detects_future_version() {
        let mut future_version = [0u8; 511];
        future_version[0] = 2u8;
        if let Err(error) = parse_memo(future_version) {
            //let string_repr = format!("{:?}", repr);
            assert_eq!(error.to_string(), "Received memo from '2', a future version of this protocol.\nPlease ensure your software is up-to-date");
        }
    }
}
