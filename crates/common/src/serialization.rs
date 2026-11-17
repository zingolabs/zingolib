//! Provides unifying interfaces for transaction management across Sapling and Orchard
use std::io::{self, Read, Write};

use byteorder::ReadBytesExt;
use tracing::{Level, event, instrument};

/// TODO: Add Doc Comment Here!
pub trait ReadableWriteable<ReadInput = (), WriteInput = ()>: Sized {
    /// TODO: Add Doc Comment Here!
    const VERSION: u8;

    /// TODO: Add Doc Comment Here!
    fn read<R: Read>(reader: R, input: ReadInput) -> io::Result<Self>;

    /// TODO: Add Doc Comment Here!
    fn write<W: Write>(&self, writer: W, input: WriteInput) -> io::Result<()>;

    /// Reads a serialized version of the struct from `reader`, and returns the
    /// struct version. Else, returns an `io::Error` with `io::ErrorKind::InvalidData`.
    #[instrument(level = "info", skip(reader))]
    fn get_version<R: Read>(reader: R) -> io::Result<u8> {
        read_version(reader, std::any::type_name::<Self>(), Self::VERSION)
    }
}

/// - Reads one byte from `reader`.
/// - Emits an `ERROR` tracing event when that byte is above `current_version`.
pub fn read_version<R: Read>(
    mut reader: R,
    type_name: &str,
    current_version: u8,
) -> io::Result<u8> {
    let version = reader.read_u8()?;
    if version > current_version {
        event!(
            Level::ERROR,
            where = type_name,
            got_version = version,
            expected_version = current_version,
            kind = ?io::ErrorKind::InvalidData,
            msg = "serialized version was written by a newer build"
        );
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{type_name} serialized version {version} was written by a newer build. this build reads up to \
                 version {current_version}."
            ),
        ));
    }

    Ok(version)
}

impl ReadableWriteable for orchard::keys::SpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let mut data = [0u8; 32];
        reader.read_exact(&mut data)?;

        Option::from(Self::from_bytes(data)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unable to deserialize a valid Orchard SpendingKey from bytes".to_owned(),
            )
        })
    }

    fn write<W: Write>(&self, mut _writer: W, _input: ()) -> io::Result<()> {
        unimplemented!()
    }
}

impl ReadableWriteable for sapling_crypto::zip32::ExtendedSpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _input: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W, _input: ()) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable for sapling_crypto::zip32::DiversifiableFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let mut fvk_bytes = [0u8; 128];
        reader.read_exact(&mut fvk_bytes)?;
        sapling_crypto::zip32::DiversifiableFullViewingKey::from_bytes(&fvk_bytes).ok_or(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Couldn't read a Sapling Diversifiable Full Viewing Key",
            ),
        )
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        writer.write_all(&self.to_bytes())
    }
}

impl ReadableWriteable for orchard::keys::FullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _input: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W, _input: ()) -> io::Result<()> {
        self.write(writer)
    }
}

#[cfg(test)]
mod read_version_tests {
    use std::io;

    use super::read_version;

    const TYPE_NAME: &str = "Type";
    const ARBITRARY_CURRENT_VERSION: u8 = 2;

    #[test]
    fn accepts_versions_up_to_the_current_version() {
        for version in [0, ARBITRARY_CURRENT_VERSION] {
            assert_eq!(
                read_version([version].as_slice(), TYPE_NAME, ARBITRARY_CURRENT_VERSION).ok(),
                Some(version)
            );
        }
    }

    #[test]
    fn refuses_a_version_above_the_current_version() {
        let newer = [ARBITRARY_CURRENT_VERSION + 1];
        assert_eq!(
            read_version(newer.as_slice(), TYPE_NAME, ARBITRARY_CURRENT_VERSION)
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }
}
