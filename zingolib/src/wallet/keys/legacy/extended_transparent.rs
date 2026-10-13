use std::fmt;
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
                accumulated | (left ^ right)
            });
        differing_bits == 0
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
    use super::{CHAIN_CODE_LEN, ExtendedPrivKey, SECRET_KEY_LEN};

    fn sample() -> ExtendedPrivKey {
        ExtendedPrivKey {
            key: [0x5a; SECRET_KEY_LEN],
            chain_code: [0xa5; CHAIN_CODE_LEN],
        }
    }

    #[test]
    fn debug_prints_a_digest_and_not_the_secret() {
        let rendered = format!("{:?}", sample());
        assert!(rendered.starts_with("LegacyExtendedKey(#"));
        assert!(!rendered.contains(&hex::encode([0x5a; SECRET_KEY_LEN])));
        assert!(!rendered.contains(&hex::encode([0xa5; CHAIN_CODE_LEN])));
    }

    #[test]
    fn equality_covers_both_fields() {
        let mut other_key = sample();
        other_key.key[SECRET_KEY_LEN - 1] ^= 1;
        let mut other_chain_code = sample();
        other_chain_code.chain_code[0] ^= 1;
        assert_eq!(sample(), sample());
        assert_ne!(sample(), other_key);
        assert_ne!(sample(), other_chain_code);
    }
}
