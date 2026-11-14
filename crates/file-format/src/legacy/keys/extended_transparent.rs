use std::fmt;
use std::io;

use zcash_encoding::Vector;
use zcash_transparent::keys;

use zl_common::serialization::ReadableWriteable;

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
mod tests;
