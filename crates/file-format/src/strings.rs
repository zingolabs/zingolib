use std::io::{self, Read};

use byteorder::{LittleEndian, ReadBytesExt};

pub use zingolib_common::wallet::serialization::write_string;

pub(crate) const MAX_STRING_LENGTH: u64 = 1 << 16;

/// ```
/// use zingolib_file_format::strings::{read_string, write_string};
///
/// let mut encoded = Vec::new();
/// write_string(&mut encoded, &"main".to_string())
///     .expect("writing into an in-memory Vec<u8> never returns an io::Error");
/// assert_eq!(
///     read_string(encoded.as_slice()).expect("write_string encoded these bytes one line above"),
///     "main"
/// );
///
/// let unbounded_length = u64::MAX.to_le_bytes();
/// assert!(read_string(unbounded_length.as_slice()).is_err());
/// ```
pub fn read_string<R: Read>(mut reader: R) -> io::Result<String> {
    let str_len = reader.read_u64::<LittleEndian>()?;
    read_string_body(reader, str_len)
}

pub(crate) fn read_string_body<R: Read>(mut reader: R, str_len: u64) -> io::Result<String> {
    if str_len > MAX_STRING_LENGTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("string length {str_len} exceeds the wallet-file maximum {MAX_STRING_LENGTH}"),
        ));
    }
    let mut str_bytes = vec![0; str_len as usize];
    reader.read_exact(&mut str_bytes)?;

    utf8_string(str_bytes)
}

pub(crate) fn utf8_string(bytes: Vec<u8>) -> io::Result<String> {
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}
