use std::io;

use zcash_encoding::Vector;

use crate::wallet::traits::ReadableWriteable;

type ChainCode = Vec<u8>;

const SECRET_KEY_LEN: usize = 32;
const COMPRESSED_PUBLIC_KEY_LEN: usize = 33;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedPrivKey {
    pub private_key: [u8; SECRET_KEY_LEN],
    pub chain_code: ChainCode,
}

impl ReadableWriteable for ExtendedPrivKey {
    const VERSION: u8 = 1;

    fn read<R: io::Read>(mut reader: R, (): ()) -> io::Result<Self> {
        Self::get_version(&mut reader)?;
        let mut private_key = [0; SECRET_KEY_LEN];
        reader.read_exact(&mut private_key)?;
        let chain_code = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        Ok(Self {
            private_key,
            chain_code,
        })
    }

    fn write<W: io::Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedPubKey {
    pub public_key: [u8; COMPRESSED_PUBLIC_KEY_LEN],
    pub chain_code: ChainCode,
}

impl ReadableWriteable for ExtendedPubKey {
    const VERSION: u8 = 1;

    fn read<R: io::Read>(mut reader: R, (): ()) -> io::Result<Self> {
        Self::get_version(&mut reader)?;
        let mut public_key = [0; COMPRESSED_PUBLIC_KEY_LEN];
        reader.read_exact(&mut public_key)?;
        let chain_code = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        Ok(Self {
            public_key,
            chain_code,
        })
    }

    fn write<W: io::Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}
