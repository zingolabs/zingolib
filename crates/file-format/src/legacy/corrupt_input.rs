use zcash_primitives::merkle_tree::write_incremental_witness;

use super::*;
use zl_common::{chain::ChainType, keys::UnifiedKeyStore};

const TRANSPARENT_OUTPUT_VERSION: u64 = 5;
const NOTE_WITH_DISCARDED_PREFIX: u8 = 1;
const NOTE_WITH_WITNESS_CACHE: u8 = 3;
const NOTE_CURRENT: u8 = 5;
const INNER_NOTE_VERSION: u8 = 1;
const RSEED_AFTER_ZIP212: u8 = 2;
const NOTE_VALUE: u64 = 1;
const ONE_RECORD: u8 = 1;
const TWO_WITNESSES: u8 = 2;
const TOP_HEIGHT_BELOW_WITNESS_COUNT: u64 = 0;

fn kind<T>(read: io::Result<T>) -> Option<io::ErrorKind> {
    read.err().map(|error| error.kind())
}

fn decode_error<T>(read: io::Result<T>) -> Option<DecodeError> {
    let error = read.err()?.into_inner()?.downcast::<DecodeError>().ok()?;
    Some(*error)
}

fn reports_a_key_error_as_its_source<T>(read: io::Result<T>) -> bool {
    read.err()
        .as_ref()
        .and_then(std::error::Error::source)
        .is_some_and(|source| source.is::<KeyError>())
}

fn transparent_output(address_len: i32, address: &[u8]) -> Vec<u8> {
    [
        &TRANSPARENT_OUTPUT_VERSION.to_le_bytes()[..],
        &address_len.to_le_bytes(),
        address,
    ]
    .concat()
}

fn sapling_note(version: u8, diversifier: sapling_crypto::Diversifier) -> Vec<u8> {
    [
        &[version][..],
        &diversifier.0,
        &[INNER_NOTE_VERSION],
        &NOTE_VALUE.to_le_bytes(),
        &[RSEED_AFTER_ZIP212],
        &[u8::MIN; 32],
    ]
    .concat()
}

fn orchard_note() -> Vec<u8> {
    [
        &[NOTE_CURRENT][..],
        &[u8::MIN; 11],
        &[INNER_NOTE_VERSION],
        &NOTE_VALUE.to_le_bytes(),
        &[u8::MIN; 32],
        &[u8::MIN; 32],
    ]
    .concat()
}

fn diversifier(on_curve: bool) -> io::Result<sapling_crypto::Diversifier> {
    (u8::MIN..=u8::MAX)
        .map(|byte| sapling_crypto::Diversifier([byte; 11]))
        .find(|diversifier| diversifier.g_d().is_some() == on_curve)
        .ok_or_else(|| io::Error::other("no single-byte diversifier has the wanted validity"))
}

fn spending_capability() -> io::Result<WalletCapability> {
    let mut capability = WalletCapability::default();
    capability.unified_key_store =
        UnifiedKeyStore::new_from_seed(ChainType::Mainnet, &[u8::MIN; 64], zip32::AccountId::ZERO)
            .map_err(io::Error::other)?;
    Ok(capability)
}

#[test]
fn a_transparent_output_with_an_empty_address_is_invalid_data() {
    assert_eq!(
        kind(TransparentOutput::read(
            transparent_output(0, b"").as_slice()
        )),
        Some(io::ErrorKind::InvalidData)
    );
}

#[test]
fn a_transparent_output_with_a_shielded_address_is_invalid_data() {
    assert_eq!(
        kind(TransparentOutput::read(
            transparent_output(1, b"z").as_slice()
        )),
        Some(io::ErrorKind::InvalidData)
    );
}

#[test]
fn a_transparent_output_with_a_negative_address_length_is_invalid_data() {
    assert_eq!(
        kind(TransparentOutput::read(
            transparent_output(-1, b"").as_slice()
        )),
        Some(io::ErrorKind::InvalidData)
    );
}

#[test]
fn outgoing_data_with_an_unbounded_address_length_is_invalid_data() {
    let unbounded = u64::MAX.to_le_bytes();
    assert_eq!(
        kind(OutgoingTxData::read_old(unbounded.as_slice())),
        Some(io::ErrorKind::InvalidData)
    );
    let versioned = [&[u8::MIN][..], &unbounded].concat();
    assert_eq!(
        kind(OutgoingTxData::read(versioned.as_slice())),
        Some(io::ErrorKind::InvalidData)
    );
}

#[test]
fn a_note_truncated_in_its_discarded_prefix_is_an_unexpected_eof() {
    let truncated = [NOTE_WITH_DISCARDED_PREFIX];
    let capability = WalletCapability::default();
    assert_eq!(
        kind(SaplingNote::read(truncated.as_slice(), (&capability, None))),
        Some(io::ErrorKind::UnexpectedEof)
    );
    assert_eq!(
        kind(OrchardNote::read(truncated.as_slice(), (&capability, None))),
        Some(io::ErrorKind::UnexpectedEof)
    );
}

#[test]
fn a_sapling_note_in_a_wallet_without_a_sapling_key_is_invalid_data() -> io::Result<()> {
    let note = sapling_note(NOTE_CURRENT, diversifier(true)?);
    assert_eq!(
        kind(SaplingNote::read(
            note.as_slice(),
            (&WalletCapability::default(), None)
        )),
        Some(io::ErrorKind::InvalidData)
    );
    Ok(())
}

#[test]
fn a_missing_viewing_key_reaches_the_caller_with_its_key_error_as_the_source() -> io::Result<()> {
    let capability = WalletCapability::default();
    let sapling = sapling_note(NOTE_CURRENT, diversifier(true)?);
    assert!(reports_a_key_error_as_its_source(SaplingNote::read(
        sapling.as_slice(),
        (&capability, None)
    )));
    assert!(reports_a_key_error_as_its_source(OrchardNote::read(
        orchard_note().as_slice(),
        (&capability, None)
    )));
    Ok(())
}

#[test]
fn a_diversifier_off_the_curve_is_the_diversifier_decode_error() -> io::Result<()> {
    let note = sapling_note(NOTE_CURRENT, diversifier(false)?);
    assert!(matches!(
        decode_error(SaplingNote::read(
            note.as_slice(),
            (&spending_capability()?, None)
        )),
        Some(DecodeError::SaplingDiversifier)
    ));
    Ok(())
}

#[test]
fn a_witness_cache_taller_than_its_top_height_is_invalid_data() -> io::Result<()> {
    let mut tree = sapling_crypto::CommitmentTree::empty();
    tree.append(sapling_crypto::Node::empty_leaf())
        .map_err(|()| io::Error::other("an empty tree refused its first leaf"))?;
    let witness = IncrementalWitness::<_, COMMITMENT_TREE_LEVELS>::from_tree(tree)
        .ok_or_else(|| io::Error::other("a one-leaf tree gave no witness"))?;
    let mut note = sapling_note(NOTE_WITH_WITNESS_CACHE, diversifier(true)?);
    note.push(TWO_WITNESSES);
    write_incremental_witness(&witness, &mut note)?;
    write_incremental_witness(&witness, &mut note)?;
    note.extend(TOP_HEIGHT_BELOW_WITNESS_COUNT.to_le_bytes());
    assert_eq!(
        kind(SaplingNote::read(
            note.as_slice(),
            (&spending_capability()?, None)
        )),
        Some(io::ErrorKind::InvalidData)
    );
    Ok(())
}

#[test]
fn a_truncated_transaction_record_propagates_its_error() {
    let truncated = [&[ONE_RECORD][..], &[u8::MIN; 32]].concat();
    assert_eq!(
        kind(TxMap::read_old(
            truncated.as_slice(),
            &WalletCapability::default()
        )),
        Some(io::ErrorKind::UnexpectedEof)
    );
}
