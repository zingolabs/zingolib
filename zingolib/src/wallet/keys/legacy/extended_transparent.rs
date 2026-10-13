use std::io;

use zcash_encoding::Vector;

use crate::wallet::traits::ReadableWriteable;

type ChainCode = Vec<u8>;

const SECRET_KEY_LEN: usize = 32;
const COMPRESSED_PUBLIC_KEY_LEN: usize = 33;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyExtendedKey<const KEY_LEN: usize> {
    pub key: [u8; KEY_LEN],
    pub chain_code: ChainCode,
}

pub type ExtendedPrivKey = LegacyExtendedKey<SECRET_KEY_LEN>;
pub type ExtendedPubKey = LegacyExtendedKey<COMPRESSED_PUBLIC_KEY_LEN>;

impl<const KEY_LEN: usize> ReadableWriteable for LegacyExtendedKey<KEY_LEN> {
    const VERSION: u8 = 1;

    fn read<R: io::Read>(mut reader: R, (): ()) -> io::Result<Self> {
        Self::get_version(&mut reader)?;
        let mut key = [0; KEY_LEN];
        reader.read_exact(&mut key)?;
        let chain_code = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        Ok(Self { key, chain_code })
    }

    fn write<W: io::Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}
