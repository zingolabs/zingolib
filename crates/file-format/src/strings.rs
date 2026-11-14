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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_utf8_is_invalid_data() {
        let err = utf8_string(vec![u8::MAX])
            .expect_err("0xff is never a byte of a UTF-8 sequence, so the bytes are not UTF-8");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn valid_utf8_decodes_to_its_string() {
        let decoded = utf8_string(b"main".to_vec())
            .expect("the bytes come from a Rust string literal, which is valid UTF-8");
        assert_eq!(decoded, "main");
    }
}
