use std::fmt;
use std::io;

use zcash_encoding::Vector;
use zcash_transparent::keys;

use crate::wallet::traits::ReadableWriteable;

pub(crate) const CHAIN_CODE_LEN: usize = 32;
pub(crate) const SECRET_KEY_LEN: usize = 32;
pub(crate) const COMPRESSED_PUBLIC_KEY_LEN: usize = 33;
pub(crate) const ACCOUNT_PUB_KEY_LEN: usize = CHAIN_CODE_LEN + COMPRESSED_PUBLIC_KEY_LEN;

const DEBUG_DIGEST_LEN: usize = 32;
const DEBUG_PERSONALIZATION: &[u8; 16] = b"Zingo_LegacyXKey";
const ACCOUNT_KEY_DEPTH: u8 = 4;
const ACCOUNT_KEY_PARENT_FINGERPRINT: [u8; 4] = [0xff; 4];
const ACCOUNT_KEY_CHILD_INDEX: u32 = 0;

#[derive(Clone)]
pub(crate) struct LegacyExtendedKey<const KEY_LEN: usize> {
    pub(crate) key: [u8; KEY_LEN],
    pub(crate) chain_code: [u8; CHAIN_CODE_LEN],
}

pub(crate) type ExtendedPrivKey = LegacyExtendedKey<SECRET_KEY_LEN>;
pub(crate) type ExtendedPubKey = LegacyExtendedKey<COMPRESSED_PUBLIC_KEY_LEN>;

pub(crate) trait AccountKey {
    type Bytes;

    const INVALID: &'static str;

    fn account_key_bytes(&self) -> Self::Bytes;

    fn parses(bytes: &Self::Bytes) -> bool;

    fn validate(&self) -> io::Result<()> {
        if Self::parses(&self.account_key_bytes()) {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidData, Self::INVALID))
        }
    }
}

impl AccountKey for ExtendedPrivKey {
    type Bytes = Vec<u8>;

    const INVALID: &'static str = "legacy transparent secret key is not a valid secp256k1 scalar";

    fn account_key_bytes(&self) -> Vec<u8> {
        let mut key_bytes = [0u8; SECRET_KEY_LEN + 1];
        key_bytes[1..].copy_from_slice(&self.key);
        let extended_key = bip32::ExtendedKey {
            prefix: bip32::Prefix::XPRV,
            attrs: bip32::ExtendedKeyAttrs {
                depth: ACCOUNT_KEY_DEPTH,
                parent_fingerprint: ACCOUNT_KEY_PARENT_FINGERPRINT,
                child_number: bip32::ChildNumber::new(ACCOUNT_KEY_CHILD_INDEX, true).expect(
                    "index 0 lies below the hardened bound, so ChildNumber::new cannot fail",
                ),
                chain_code: self.chain_code,
            },
            key_bytes,
        };
        bs58::decode(extended_key.to_string())
            .with_check(None)
            .into_vec()
            .expect("bip32 base58check-encoded this string one line above, so decoding cannot fail")
            .split_off(bip32::Prefix::LENGTH)
    }

    fn parses(bytes: &Vec<u8>) -> bool {
        keys::AccountPrivKey::from_bytes(bytes).is_some()
    }
}

impl AccountKey for ExtendedPubKey {
    type Bytes = [u8; ACCOUNT_PUB_KEY_LEN];

    const INVALID: &'static str =
        "legacy transparent public key is not a compressed secp256k1 point";

    fn account_key_bytes(&self) -> [u8; ACCOUNT_PUB_KEY_LEN] {
        let mut bytes = [0u8; ACCOUNT_PUB_KEY_LEN];
        bytes[..CHAIN_CODE_LEN].copy_from_slice(&self.chain_code);
        bytes[CHAIN_CODE_LEN..].copy_from_slice(&self.key);
        bytes
    }

    fn parses(bytes: &[u8; ACCOUNT_PUB_KEY_LEN]) -> bool {
        keys::AccountPubKey::deserialize(bytes).is_ok()
    }
}

impl<const KEY_LEN: usize> fmt::Debug for LegacyExtendedKey<KEY_LEN> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digest = blake2b_simd::Params::new()
            .hash_length(DEBUG_DIGEST_LEN)
            .personal(DEBUG_PERSONALIZATION)
            .to_state()
            .update(&self.key)
            .update(&self.chain_code)
            .finalize();
        write!(formatter, "LegacyExtendedKey(#{})", digest.to_hex())
    }
}

impl<const KEY_LEN: usize> ReadableWriteable for LegacyExtendedKey<KEY_LEN>
where
    Self: AccountKey,
{
    const VERSION: u8 = 1;

    fn read<R: io::Read>(mut reader: R, (): ()) -> io::Result<Self> {
        Self::get_version(&mut reader)?;
        let mut key = [0; KEY_LEN];
        reader.read_exact(&mut key)?;
        let chain_code = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?
            .try_into()
            .map_err(|bytes: Vec<u8>| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "legacy chain code holds {} bytes, expected {CHAIN_CODE_LEN}",
                        bytes.len()
                    ),
                )
            })?;
        let extended_key = Self { key, chain_code };
        extended_key.validate()?;
        Ok(extended_key)
    }

    fn write<W: io::Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use byteorder::WriteBytesExt;
    use zcash_encoding::Vector;

    use super::AccountKey;
    use crate::wallet::error::KeyError;
    use crate::wallet::keys::legacy;
    use crate::wallet::traits::ReadableWriteable;

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
        let orchard_key = orchard::keys::SpendingKey::from_zip32_seed(
            &seed,
            TEST_COIN_TYPE,
            zip32::AccountId::ZERO,
        )
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
}
