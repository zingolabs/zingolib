use std::io;

use byteorder::WriteBytesExt;
use zcash_encoding::Vector;

use super::AccountKey;
use crate::legacy::keys as legacy;
use zl_common::keys::KeyError;
use zl_common::serialization::ReadableWriteable;

const SECP256K1_GENERATOR_COMPRESSED: &str =
    "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const INVALID_POINT_TAG: u8 = 0x05;
const TEST_COIN_TYPE: u32 = 1;

fn sample() -> super::ExtendedPrivKey {
    super::ExtendedPrivKey {
        key: [0x5a; super::SECRET_KEY_LEN],
        chain_code: [0xa5; super::CHAIN_CODE_LEN],
    }
}

fn encode_version_one(key: &[u8], chain_code: &[u8]) -> Vec<u8> {
    let mut bytes = vec![super::ExtendedPubKey::VERSION];
    bytes.extend_from_slice(key);
    Vector::write(&mut bytes, chain_code, |writer, byte| {
        writer.write_u8(*byte)
    })
    .expect("writing into an in-memory Vec<u8> never returns an io::Error");
    bytes
}

fn invalid_data_message<T: std::fmt::Debug>(outcome: io::Result<T>) -> String {
    let Err(error) = outcome else {
        panic!("the bytes must not parse");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    error.to_string()
}

#[test]
fn debug_prints_a_digest_and_not_the_secret() {
    let rendered = format!("{:?}", sample());
    assert!(rendered.starts_with("LegacyExtendedKey(#"));
    assert!(!rendered.contains(&hex::encode([0x5a; super::SECRET_KEY_LEN])));
    assert!(!rendered.contains(&hex::encode([0xa5; super::CHAIN_CODE_LEN])));
}

#[test]
fn view_only_key_round_trips_into_the_ufvk_p2pkh_item() -> Result<(), KeyError> {
    let generator: [u8; super::COMPRESSED_PUBLIC_KEY_LEN] =
        hex::decode(SECP256K1_GENERATOR_COMPRESSED)
            .expect("the generator constant is valid hex")
            .try_into()
            .expect("the generator constant is 33 bytes of hex");
    let stored = super::ExtendedPubKey {
        key: generator,
        chain_code: [0xa5; super::CHAIN_CODE_LEN],
    };

    let bytes = encode_version_one(&stored.key, &stored.chain_code);
    let read_back = super::ExtendedPubKey::read(bytes.as_slice(), ())?;
    assert_eq!(read_back.key, stored.key);
    assert_eq!(read_back.chain_code, stored.chain_code);

    let sapling_dfvk = sapling_crypto::zip32::ExtendedSpendingKey::master(&[0u8; 32])
        .expect("a 32-byte seed derives a valid sapling master key")
        .to_diversifiable_full_viewing_key();
    let ufvk = legacy::legacy_fvks_to_ufvk(
        None,
        Some(&sapling_dfvk),
        Some(&read_back),
        &zcash_protocol::consensus::MAIN_NETWORK,
    )?;
    let p2pkh = ufvk
        .p2pkh()
        .expect("legacy_fvks_to_ufvk pushed the P2PKH item this test supplied");

    assert_eq!(p2pkh.serialize(), stored.account_key_bytes());
    Ok(())
}

#[test]
fn spending_key_reads_and_reaches_the_usk() -> Result<(), KeyError> {
    let stored = sample();
    let bytes = encode_version_one(&stored.key, &stored.chain_code);
    let read_back = super::ExtendedPrivKey::read(bytes.as_slice(), ())?;
    assert_eq!(read_back.key, stored.key);
    assert_eq!(read_back.chain_code, stored.chain_code);

    let seed = [0u8; 32];
    let orchard_key =
        orchard::keys::SpendingKey::from_zip32_seed(&seed, TEST_COIN_TYPE, zip32::AccountId::ZERO)
            .expect("the zero seed derives a valid orchard spending key");
    let sapling_key = sapling_crypto::zip32::ExtendedSpendingKey::master(&seed)
        .expect("a 32-byte seed derives a valid sapling master key");
    let usk = legacy::legacy_sks_to_usk(&orchard_key, &sapling_key, &read_back)?;
    assert_eq!(usk.transparent().to_bytes(), stored.account_key_bytes());
    Ok(())
}

#[test]
fn a_short_chain_code_is_invalid_data() {
    let bytes = encode_version_one(
        &[0x5a; super::SECRET_KEY_LEN],
        &[0xa5; super::CHAIN_CODE_LEN - 1],
    );
    let message = invalid_data_message(super::ExtendedPrivKey::read(bytes.as_slice(), ()));
    assert!(message.contains("chain code"), "{message}");
}

#[test]
fn an_invalid_point_is_rejected_at_read() {
    let mut key = [0u8; super::COMPRESSED_PUBLIC_KEY_LEN];
    key[0] = INVALID_POINT_TAG;
    let bytes = encode_version_one(&key, &[0xa5; super::CHAIN_CODE_LEN]);
    let message = invalid_data_message(super::ExtendedPubKey::read(bytes.as_slice(), ()));
    assert!(message.contains("public key"), "{message}");
}

#[test]
fn an_out_of_range_scalar_is_rejected_at_read() {
    let bytes = encode_version_one(
        &[0xff; super::SECRET_KEY_LEN],
        &[0xa5; super::CHAIN_CODE_LEN],
    );
    let message = invalid_data_message(super::ExtendedPrivKey::read(bytes.as_slice(), ()));
    assert!(message.contains("secret key"), "{message}");
}
