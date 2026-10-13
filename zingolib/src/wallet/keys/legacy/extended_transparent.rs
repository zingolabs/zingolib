use std::fmt;
use std::hint;
use std::io;

use zcash_encoding::Vector;

use crate::wallet::traits::ReadableWriteable;

pub(crate) const CHAIN_CODE_LEN: usize = 32;
pub(crate) const SECRET_KEY_LEN: usize = 32;
pub(crate) const COMPRESSED_PUBLIC_KEY_LEN: usize = 33;

const DEBUG_DIGEST_LEN: usize = 32;
const DEBUG_PERSONALIZATION: &[u8; 16] = b"Zingo_LegacyXKey";

#[derive(Clone)]
pub struct LegacyExtendedKey<const KEY_LEN: usize> {
    pub key: [u8; KEY_LEN],
    pub chain_code: [u8; CHAIN_CODE_LEN],
}

pub type ExtendedPrivKey = LegacyExtendedKey<SECRET_KEY_LEN>;
pub type ExtendedPubKey = LegacyExtendedKey<COMPRESSED_PUBLIC_KEY_LEN>;

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

impl<const KEY_LEN: usize> PartialEq for LegacyExtendedKey<KEY_LEN> {
    fn eq(&self, other: &Self) -> bool {
        let differing_bits = self
            .key
            .iter()
            .zip(other.key.iter())
            .chain(self.chain_code.iter().zip(other.chain_code.iter()))
            .fold(0u8, |accumulated, (left, right)| {
                accumulated | hint::black_box(left ^ right)
            });
        hint::black_box(differing_bits) == 0
    }
}

impl<const KEY_LEN: usize> Eq for LegacyExtendedKey<KEY_LEN> {}

impl<const KEY_LEN: usize> ReadableWriteable for LegacyExtendedKey<KEY_LEN> {
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
        Ok(Self { key, chain_code })
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

    use crate::wallet::error::KeyError;
    use crate::wallet::keys::legacy;
    use crate::wallet::traits::ReadableWriteable;

    const SECP256K1_GENERATOR_COMPRESSED: &str =
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

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

    #[test]
    fn debug_prints_a_digest_and_not_the_secret() {
        let rendered = format!("{:?}", sample());
        assert!(rendered.starts_with("LegacyExtendedKey(#"));
        assert!(!rendered.contains(&hex::encode([0x5a; super::SECRET_KEY_LEN])));
        assert!(!rendered.contains(&hex::encode([0xa5; super::CHAIN_CODE_LEN])));
    }

    #[test]
    fn equality_covers_both_fields() {
        let mut other_key = sample();
        other_key.key[super::SECRET_KEY_LEN - 1] ^= 1;
        let mut other_chain_code = sample();
        other_chain_code.chain_code[0] ^= 1;
        assert_eq!(sample(), sample());
        assert_ne!(sample(), other_key);
        assert_ne!(sample(), other_chain_code);
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
        assert_eq!(read_back, stored);

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

        let mut expected = stored.chain_code.to_vec();
        expected.extend_from_slice(&stored.key);
        assert_eq!(p2pkh.serialize(), expected);
        Ok(())
    }

    #[test]
    fn a_short_chain_code_is_invalid_data() {
        let bytes = encode_version_one(
            &[0x5a; super::SECRET_KEY_LEN],
            &[0xa5; super::CHAIN_CODE_LEN - 1],
        );
        let Err(error) = super::ExtendedPrivKey::read(bytes.as_slice(), ()) else {
            panic!("a 31-byte chain code must not parse");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
