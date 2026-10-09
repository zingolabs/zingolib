//! Zingo-Memo
//!
//! Utilities for procedural creation and parsing of the Memo field.
//!
//! These memos are not directly exposed to the user,
//! but instead write down UAs on-chain for recovery after rescan.

#![warn(missing_docs)]
use std::io::{self, Read, Write};

use zcash_address::unified::{
    Address, Container, Encoding, MetadataItem, MetadataTypecode, Receiver, Revision, Typecode,
    Uitem,
};
use zcash_encoding::{CompactSize, Vector};
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::consensus::Parameters;

/// A parsed memo.
/// The main use-case for this is to record the UAs that a foreign recipient provided,
/// as the blockchain only records the pool-specific receiver corresponding to the key we sent with.
/// Version 1 also framed the indexes of the refund addresses a send to a TEX
/// address used. Nothing writes those any more; they are still read from the
/// memos of older releases.
#[derive(Debug)]
pub enum ParsedMemo {
    /// the memo including only a list of unified addresses
    Version0 {
        /// The list of unified addresses
        uas: Vec<UnifiedAddress>,
    },
    /// the memo including unified addresses and refund address indexes
    Version1 {
        /// the list of unified addresses
        uas: Vec<UnifiedAddress>,
        /// The refund address indexes an older release recorded. This release
        /// writes an empty list.
        rejection_address_indexes: Vec<u32>,
    },
    /// the memo including unified addresses with their ZIP 316 revision and
    /// metadata items. Refund address indexes are no longer recorded.
    Version2 {
        /// the list of unified addresses
        uas: Vec<UnifiedAddress>,
    },
}

impl ParsedMemo {
    /// The unified addresses the memo records, whichever version carried them.
    #[must_use]
    pub fn into_unified_addresses(self) -> Vec<UnifiedAddress> {
        match self {
            ParsedMemo::Version0 { uas }
            | ParsedMemo::Version1 { uas, .. }
            | ParsedMemo::Version2 { uas } => uas,
        }
    }
}

/// The length of the memo field.
pub const MEMO_LEN: usize = 511;

/// The memo versions this crate reads; version 0 is no longer written.
const VERSION_0: usize = 0;
const VERSION_1: usize = 1;
const VERSION_2: usize = 2;

/// The length of the refund address list a version 1 memo frames: nothing
/// writes a refund address index any more.
const NO_REFUND_ADDRESSES: usize = 0;

/// A memo packed by [`create_wallet_internal_memo`]: the memo field, and how
/// many of the given addresses it records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedMemo {
    /// The memo field.
    pub bytes: [u8; MEMO_LEN],
    /// How many addresses, counted from the first given, the memo records.
    /// Fewer than were given means the rest did not fit.
    pub recorded: usize,
}

/// The ZIP 316 revision numbers a version 2 memo writes before an address's items.
const REVISION_0: usize = 0;
const REVISION_2: usize = 2;

/// The byte length of an expiry height metadata item.
const EXPIRY_HEIGHT_LEN: usize = 4;
/// The byte length of an expiry time metadata item.
const EXPIRY_TIME_LEN: usize = 8;

/// How a memo version writes an address.
#[derive(Clone, Copy)]
enum AddressFormat {
    /// Receivers alone, the form of versions 0 and 1.
    Raw,
    /// The ZIP 316 revision and every item, the form of version 2.
    WithRevision,
}

/// Frames one memo, the version and the addresses in `format`, packed into
/// the memo field. Errors when the framing outgrows the field.
fn pack_memo(
    version: usize,
    format: AddressFormat,
    consensus_parameters: &impl Parameters,
    uas: &[UnifiedAddress],
) -> io::Result<[u8; MEMO_LEN]> {
    fit_memo(frame_memo(version, format, consensus_parameters, uas)?)
}

/// Frames one memo without regard to the field's length.
fn frame_memo(
    version: usize,
    format: AddressFormat,
    consensus_parameters: &impl Parameters,
    uas: &[UnifiedAddress],
) -> io::Result<Vec<u8>> {
    let mut memo_bytes_vec = Vec::new();
    CompactSize::write(&mut memo_bytes_vec, version)?;
    Vector::write(&mut memo_bytes_vec, uas, |w, ua| match format {
        AddressFormat::Raw => write_unified_address_to_raw_encoding(consensus_parameters, ua, w),
        AddressFormat::WithRevision => {
            write_unified_address_with_revision(consensus_parameters, ua, w)
        }
    })?;
    // Version 1 frames a list of refund address indexes after the addresses.
    // The list is part of the format its readers expect, so it is written
    // empty.
    if version == VERSION_1 {
        CompactSize::write(&mut memo_bytes_vec, NO_REFUND_ADDRESSES)?;
    }
    Ok(memo_bytes_vec)
}

/// Lays a framed memo into the memo field, or errors when it does not fit.
fn fit_memo(framed: Vec<u8>) -> io::Result<[u8; MEMO_LEN]> {
    let mut memo_bytes = [0u8; MEMO_LEN];
    if framed.len() > MEMO_LEN {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Too many addresses to fit in memo field",
        ))
    } else {
        memo_bytes[..framed.len()].copy_from_slice(framed.as_slice());
        Ok(memo_bytes)
    }
}

/// Packs a list of UAs into a version 1 memo.
/// Note that a UA's raw representation is 1 byte for length, +21 for a T-receiver,
/// +44 for a Sapling receiver, and +44 for an Orchard receiver. This totals a maximum
/// of 110 bytes per UA, and attempting to write more than 510 bytes will cause an error.
/// The refund address list the format frames after the addresses is written
/// empty, one byte.
///
/// A version 1 memo carries receivers alone, so an address that only ZIP 316
/// Revision 2 can represent (one with expiry metadata, or with no shielded
/// receiver) is refused rather than recorded incompletely.
///
/// The memo version follows the addresses, so a caller has no version to
/// choose: [`create_wallet_internal_memo`] writes version 1 for every list
/// this function accepts and version 2 for the lists it refuses.
#[deprecated(note = "the memo version follows the addresses; use create_wallet_internal_memo")]
pub fn create_wallet_internal_memo_version_1(
    consensus_parameters: &impl Parameters,
    uas: &[UnifiedAddress],
) -> io::Result<[u8; MEMO_LEN]> {
    pack_memo(VERSION_1, AddressFormat::Raw, consensus_parameters, uas)
}

/// Packs a list of UAs into a memo at the lowest version that records every
/// address exactly: version 1 when every address is a ZIP 316 Revision 0
/// address, version 2 otherwise.
///
/// Version 1 is preferred where it suffices because every reader understands it,
/// including the releases that predate version 2. Version 2 records each
/// address's ZIP 316 revision and every item it carries, metadata included, so
/// the address read back is the one that was paid. There each address costs one
/// byte for its revision beyond the version 1 cost, plus its metadata items: 6
/// bytes for an expiry height and 10 for an expiry time.
///
/// The memo field is finite and a send is not. When the addresses outgrow it,
/// the memo records the longest prefix of them that fits, and
/// [`PackedMemo::recorded`] says how many that is; the caller decides what to
/// say about the rest. An error here means an address could not be encoded at
/// all, never that it did not fit.
pub fn create_wallet_internal_memo(
    consensus_parameters: &impl Parameters,
    uas: &[UnifiedAddress],
) -> io::Result<PackedMemo> {
    for recorded in (0..=uas.len()).rev() {
        let framed = frame_lowest_version(consensus_parameters, &uas[..recorded])?;
        if framed.len() <= MEMO_LEN {
            return Ok(PackedMemo {
                bytes: fit_memo(framed)?,
                recorded,
            });
        }
    }
    unreachable!("a memo of no addresses always fits")
}

/// Frames `uas` at the lowest version that records every one of them exactly.
fn frame_lowest_version(
    consensus_parameters: &impl Parameters,
    uas: &[UnifiedAddress],
) -> io::Result<Vec<u8>> {
    let needs_revision_2 = uas
        .iter()
        .map(|ua| unified_container(consensus_parameters, ua))
        .collect::<io::Result<Vec<_>>>()?
        .iter()
        .any(|container| container.revision() != Revision::R0);
    if needs_revision_2 {
        frame_memo(
            VERSION_2,
            AddressFormat::WithRevision,
            consensus_parameters,
            uas,
        )
    } else {
        frame_memo(VERSION_1, AddressFormat::Raw, consensus_parameters, uas)
    }
}

/// Attempts to parse the 511 bytes of a zingo memo
pub fn parse_zingo_memo(memo: [u8; MEMO_LEN]) -> io::Result<ParsedMemo> {
    let mut reader: &[u8] = &memo;
    match CompactSize::read_t(&mut reader)? {
        VERSION_0 => Ok(ParsedMemo::Version0 {
            uas: Vector::read(&mut reader, read_address(AddressFormat::Raw))?,
        }),
        VERSION_1 => Ok(ParsedMemo::Version1 {
            uas: Vector::read(&mut reader, read_address(AddressFormat::Raw))?,
            rejection_address_indexes: Vector::read(&mut reader, read_index)?,
        }),
        VERSION_2 => Ok(ParsedMemo::Version2 {
            uas: Vector::read(&mut reader, read_address(AddressFormat::WithRevision))?,
        }),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Received encoded memo data from a different wallet or a future wallet version.\n\
            Please ensure your software is up-to-date",
        )),
    }
}

// The memo's element readers take the reader by reference, which is what
// `Vector::read` hands its element function. The public readers take it by
// value, and a function item over a by-value reference reader is fixed to one
// lifetime, so it cannot be passed there directly.
fn read_index<R: Read>(reader: &mut R) -> io::Result<u32> {
    CompactSize::read_t(reader)
}

fn read_address<R: Read>(format: AddressFormat) -> impl Fn(&mut R) -> io::Result<UnifiedAddress> {
    move |reader| match format {
        AddressFormat::Raw => read_unified_address_from_raw_encoding(reader),
        AddressFormat::WithRevision => read_unified_address_with_revision(reader),
    }
}

/// The error a memo's bytes earn when they do not describe what they claim.
fn invalid_data<E: std::error::Error + Send + Sync + 'static>(error: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// The unified container of a UA with every receiver it carries, at the lowest
/// ZIP 316 revision that can represent it.
fn unified_container(
    consensus_parameters: &impl Parameters,
    ua: &UnifiedAddress,
) -> io::Result<Address> {
    let encoded = ua.encode_receiver_preserving(consensus_parameters);
    let (_network, _revision, container) = Address::decode(&encoded).map_err(invalid_data)?;
    Ok(container)
}

/// Writes one item as its typecode, its length, and its bytes.
fn write_item<W: Write>(mut writer: W, typecode: u32, data: &[u8]) -> io::Result<()> {
    CompactSize::write(&mut writer, typecode as usize)?;
    CompactSize::write(&mut writer, data.len())?;
    writer.write_all(data)
}

/// Reads a `CompactSize` count of items, each as its typecode, its length, and
/// its bytes, through `decode`.
fn read_items<R: Read, T>(
    reader: R,
    decode: impl Fn(usize, Vec<u8>) -> io::Result<T>,
) -> io::Result<Vec<T>> {
    Vector::read(reader, |mut r| {
        let typecode: usize = CompactSize::read_t(&mut r)?;
        let len: usize = CompactSize::read_t(&mut r)?;
        let mut data = vec![0; len];
        r.read_exact(&mut data)?;
        decode(typecode, data)
    })
}

/// A helper function to encode a UA as a `CompactSize` specifying the number
/// of receivers, followed by the UA's raw encoding as specified in
/// <https://zips.z.cash/zip-0316#encoding-of-unified-addresses>
///
/// Only a ZIP 316 Revision 0 address has this encoding; an address that needs
/// Revision 2 is refused, since its metadata would be lost.
pub fn write_unified_address_to_raw_encoding<W: Write>(
    consensus_parameters: &impl Parameters,
    ua: &UnifiedAddress,
    writer: W,
) -> io::Result<()> {
    let container = unified_container(consensus_parameters, ua)?;
    if container.revision() != Revision::R0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the address needs ZIP 316 Revision 2, which only a version 2 memo records",
        ));
    }
    let receivers = container.items();
    Vector::write(writer, &receivers, |w, receiver| {
        let (typecode, data) = receiver_bytes(receiver);
        write_item(w, typecode, data)
    })
}

/// A receiver's typecode and raw bytes.
fn receiver_bytes(receiver: &Receiver) -> (u32, &[u8]) {
    match receiver {
        Receiver::Orchard(data) => (3, data),
        Receiver::Sapling(data) => (2, data),
        Receiver::P2sh(data) => (1, data),
        Receiver::P2pkh(data) => (0, data),
        Receiver::Unknown { typecode, data } => (*typecode, data.as_slice()),
    }
}

/// A helper function to decode a UA from a `CompactSize` specifying the number of
/// receivers, followed by the UA's raw encoding as specified in
/// <https://zips.z.cash/zip-0316#encoding-of-unified-addresses>
pub fn read_unified_address_from_raw_encoding<R: Read>(reader: R) -> io::Result<UnifiedAddress> {
    let receivers = read_items(reader, decode_receiver)?;
    unified_address_from_items(
        Revision::R0,
        receivers.into_iter().map(Uitem::Data).collect(),
    )
}

/// Writes a UA as its ZIP 316 revision number followed by a `CompactSize`
/// count of items and each item's typecode, length, and bytes, metadata items
/// included, in the container's encoding order.
pub fn write_unified_address_with_revision<W: Write>(
    consensus_parameters: &impl Parameters,
    ua: &UnifiedAddress,
    mut writer: W,
) -> io::Result<()> {
    let container = unified_container(consensus_parameters, ua)?;
    let revision = match container.revision() {
        Revision::R0 => REVISION_0,
        Revision::R2 => REVISION_2,
    };
    CompactSize::write(&mut writer, revision)?;
    Vector::write(writer, container.items_as_parsed(), |w, item| match item {
        Uitem::Data(receiver) => {
            let (typecode, data) = receiver_bytes(receiver);
            write_item(w, typecode, data)
        }
        Uitem::Metadata(metadata) => write_item(
            w,
            metadata.combined_typecode().typecode_value(),
            &metadata.data(),
        ),
    })
}

/// Reads a UA written by [`write_unified_address_with_revision`].
pub fn read_unified_address_with_revision<R: Read>(mut reader: R) -> io::Result<UnifiedAddress> {
    let revision = match CompactSize::read_t(&mut reader)? {
        REVISION_0 => Revision::R0,
        REVISION_2 => Revision::R2,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown ZIP 316 revision {other} in memo"),
            ));
        }
    };
    unified_address_from_items(revision, read_items(reader, decode_item)?)
}

fn unified_address_from_items(
    revision: Revision,
    items: Vec<Uitem<Receiver>>,
) -> io::Result<UnifiedAddress> {
    let address = Address::try_from_items(revision, items).map_err(invalid_data)?;
    UnifiedAddress::try_from(address).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Classifies a typecode as data or metadata and parses the item accordingly.
fn decode_item(typecode: usize, data: Vec<u8>) -> io::Result<Uitem<Receiver>> {
    let typecode_value = u32::try_from(typecode).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("typecode {typecode} exceeds the unified typecode range"),
        )
    })?;
    match Typecode::try_from(typecode_value).map_err(invalid_data)? {
        Typecode::Data(_) => Ok(Uitem::Data(decode_receiver(typecode, data)?)),
        Typecode::Metadata(metadata_typecode) => {
            Ok(Uitem::Metadata(decode_metadata(metadata_typecode, data)?))
        }
    }
}

fn decode_metadata(typecode: MetadataTypecode, data: Vec<u8>) -> io::Result<MetadataItem> {
    fn fixed<const N: usize>(name: &str, data: Vec<u8>) -> io::Result<[u8; N]> {
        <[u8; N]>::try_from(data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{name} metadata indicates {N} bytes, found length of {}",
                    e.len()
                ),
            )
        })
    }
    Ok(match typecode {
        MetadataTypecode::ExpiryHeight => {
            MetadataItem::ExpiryHeight(u32::from_le_bytes(fixed::<EXPIRY_HEIGHT_LEN>(
                "Expiry height",
                data,
            )?))
        }
        MetadataTypecode::ExpiryTime => {
            MetadataItem::ExpiryTime(u64::from_le_bytes(fixed::<EXPIRY_TIME_LEN>(
                "Expiry time",
                data,
            )?))
        }
        MetadataTypecode::Unknown(typecode) => MetadataItem::Unknown { typecode, data },
    })
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
mod test_vectors;

#[cfg(test)]
mod tests {
    use super::test_vectors as zingomemo_vectors;
    use super::*;
    use test_vectors::TestVector;
    use zcash_protocol::consensus::{BlockHeight, MAIN_NETWORK};

    fn get_serialiazed_ua(test_vector: &TestVector) -> (UnifiedAddress, Vec<u8>) {
        let zcash_keys::address::Address::Unified(ua) =
            zcash_keys::address::Address::decode(&MAIN_NETWORK, test_vector.unified_addr).unwrap()
        else {
            panic!("Couldn't decode test_vector UA")
        };
        let mut serialized_ua = Vec::new();
        write_unified_address_to_raw_encoding(&MAIN_NETWORK, &ua, &mut serialized_ua).unwrap();
        (*ua, serialized_ua)
    }
    /// A Revision 0 address is written as a version 1 memo, the framing every
    /// reader knows, with an empty refund address list.
    #[test]
    fn a_revision_0_address_round_trips_through_version_1() {
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, _) = get_serialiazed_ua(test_vector);
            let packed = create_wallet_internal_memo(&MAIN_NETWORK, std::slice::from_ref(&ua))
                .expect("To create version 1 bytes");
            let ParsedMemo::Version1 {
                uas,
                rejection_address_indexes,
            } = parse_zingo_memo(packed.bytes).expect("To succeed in parse.")
            else {
                panic!("a Revision 0 address is written as version 1")
            };
            assert_eq!(uas, [ua]);
            assert!(rejection_address_indexes.is_empty());
        }
    }
    #[test]
    fn round_trip_ser_deser() {
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, serialized_ua) = get_serialiazed_ua(test_vector);
            assert_eq!(
                ua,
                read_unified_address_from_raw_encoding(&*serialized_ua).unwrap()
            );
        }
    }

    /// The same receivers as `ua`, with expiry metadata that only ZIP 316
    /// Revision 2 can carry.
    fn with_expiry(ua: &UnifiedAddress) -> UnifiedAddress {
        UnifiedAddress::from_receivers(
            ua.orchard().copied(),
            ua.sapling().cloned(),
            ua.transparent().copied(),
            Some(BlockHeight::from_u32(3_000_000)),
            Some(1_900_000_000),
        )
        .expect("the vector has a receiver")
    }

    /// A version 2 memo reads back an expiring address with its metadata, and
    /// a transparent-only address, which Revision 0 cannot carry at all.
    #[test]
    fn version_2_records_revision_2_addresses_exactly() {
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, _) = get_serialiazed_ua(test_vector);
            let mut uas = vec![with_expiry(&ua)];
            if let Some(transparent) = ua.transparent().copied() {
                uas.push(
                    UnifiedAddress::from_receivers(None, None, Some(transparent), None, None)
                        .expect("a transparent receiver suffices"),
                );
            }
            let packed = create_wallet_internal_memo(&MAIN_NETWORK, &uas)
                .expect("to create version 2 bytes");
            assert_eq!(packed.recorded, uas.len());
            let ParsedMemo::Version2 { uas: parsed } =
                parse_zingo_memo(packed.bytes).expect("to succeed in parse")
            else {
                panic!("a version 2 memo parses as version 2")
            };
            assert_eq!(parsed, uas);
        }
    }

    /// A Revision 2 address among Revision 0 ones lifts the memo to version 2.
    #[test]
    fn a_revision_2_address_lifts_the_memo_to_version_2() {
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, _) = get_serialiazed_ua(test_vector);
            let expiring = with_expiry(&ua);
            let packed =
                create_wallet_internal_memo(&MAIN_NETWORK, &[ua.clone(), expiring]).unwrap();
            assert!(matches!(
                parse_zingo_memo(packed.bytes).unwrap(),
                ParsedMemo::Version2 { .. }
            ));
        }
    }

    /// Addresses that outgrow the memo field are left off the end rather than
    /// failing the memo: the field records the longest prefix that fits, and
    /// the count says how many.
    #[test]
    fn an_overflowing_list_records_the_prefix_that_fits() {
        // A P2PKH receiver is the smallest an address can be, so this many
        // addresses overflow the field whatever receivers the vector carries.
        const SMALLEST_ADDRESS_LEN: usize = 20;
        let too_many = MEMO_LEN / SMALLEST_ADDRESS_LEN + 1;
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, _) = get_serialiazed_ua(test_vector);
            let uas = vec![with_expiry(&ua); too_many];
            let packed = create_wallet_internal_memo(&MAIN_NETWORK, &uas).unwrap();
            assert!(packed.recorded > 0);
            assert!(packed.recorded < too_many);
            assert_eq!(
                parse_zingo_memo(packed.bytes)
                    .unwrap()
                    .into_unified_addresses(),
                uas[..packed.recorded]
            );
            let one_more =
                create_wallet_internal_memo(&MAIN_NETWORK, &uas[..packed.recorded + 1]).unwrap();
            assert_eq!(one_more.recorded, packed.recorded);
        }
    }

    /// The raw encoding, which versions 0 and 1 carry, refuses an address it
    /// would record incompletely instead of dropping its metadata.
    #[test]
    fn the_raw_encoding_refuses_a_revision_2_address() {
        for test_vector in zingomemo_vectors::UA_TEST_VECTORS {
            let (ua, _) = get_serialiazed_ua(test_vector);
            let error = write_unified_address_to_raw_encoding(
                &MAIN_NETWORK,
                &with_expiry(&ua),
                &mut Vec::new(),
            )
            .expect_err("an expiring address has no raw encoding");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }
}
