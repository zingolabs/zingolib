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

    /// TODO: Add Doc Comment Here!
    #[instrument(level = "info", skip(reader))]
    fn get_version<R: Read>(mut reader: R) -> io::Result<u8> {
        let external_version = reader.read_u8()?;
        if external_version > Self::VERSION {
            event!(
                Level::ERROR,
                where = std::any::type_name::<Self>(),
                got_version = external_version,
                expected_version = Self::VERSION,
                kind = ?io::ErrorKind::InvalidData,
                msg = "Struct version is from a future version of zingo"
            );
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Struct version \"{}\" is from future version of zingo",
                    external_version,
                ),
            ))
        } else {
            Ok(external_version)
        }
    }
}
