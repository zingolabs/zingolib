//! Byte-level primitives shared by the era modules.
//!
//! The wallet writer's history uses three length disciplines, and telling
//! them apart is much of the census, so each has its own helper here:
//! little-endian scalars (byteorder style), Bitcoin CompactSize counts (as
//! `zcash_encoding::Vector` writes them), and u64-length strings (as the
//! historical `read_string`/`write_string` pair framed them). Every helper
//! appends to `out`.

/// Append one byte.
pub fn push_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

/// Append a little-endian u16.
pub fn push_u16_le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append a little-endian u32.
pub fn push_u32_le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append a little-endian u64.
pub fn push_u64_le(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append a little-endian i32.
pub fn push_i32_le(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append a little-endian i64.
pub fn push_i64_le(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Append raw bytes.
pub fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(bytes);
}

/// Append a Bitcoin CompactSize, the count encoding `zcash_encoding::Vector`
/// (and its librustzcash predecessors) writes before vector elements.
pub fn push_compact_size(out: &mut Vec<u8>, n: u64) {
    match n {
        0..=0xFC => out.push(n as u8),
        0xFD..=0xFFFF => {
            out.push(0xFD);
            push_u16_le(out, n as u16);
        }
        0x1_0000..=0xFFFF_FFFF => {
            out.push(0xFE);
            push_u32_le(out, n as u32);
        }
        _ => {
            out.push(0xFF);
            push_u64_le(out, n);
        }
    }
}

/// Append a byte vector in `Vector` discipline: CompactSize count, then the
/// bytes as u8 elements.
pub fn push_compact_vec_u8(out: &mut Vec<u8>, bytes: &[u8]) {
    push_compact_size(out, bytes.len() as u64);
    push_bytes(out, bytes);
}

/// Append a string in the historical `write_string` discipline: u64
/// little-endian byte length, then the UTF-8 bytes. This is the framing whose
/// untrusted-length read (`read_string`) is the census's motivating defect.
pub fn push_u64_string(out: &mut Vec<u8>, s: &str) {
    push_u64_le(out, s.len() as u64);
    push_bytes(out, s.as_bytes());
}

/// Append an `Optional` None marker (`zcash_encoding::Optional` writes a 0u8).
pub fn push_optional_none(out: &mut Vec<u8>) {
    out.push(0);
}

/// Append an `Optional` Some marker (a 1u8); the caller appends the payload.
pub fn push_optional_some(out: &mut Vec<u8>) {
    out.push(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_size_uses_the_bitcoin_thresholds() {
        let mut out = Vec::new();
        push_compact_size(&mut out, 0xFC);
        assert_eq!(out, [0xFC]);

        out.clear();
        push_compact_size(&mut out, 0xFD);
        assert_eq!(out, [0xFD, 0xFD, 0x00]);

        out.clear();
        push_compact_size(&mut out, 0x1_0000);
        assert_eq!(out, [0xFE, 0x00, 0x00, 0x01, 0x00]);

        out.clear();
        push_compact_size(&mut out, 0x1_0000_0000);
        assert_eq!(out, [0xFF, 0, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn u64_string_frames_length_then_utf8() {
        let mut out = Vec::new();
        push_u64_string(&mut out, "main");
        assert_eq!(out, [4, 0, 0, 0, 0, 0, 0, 0, b'm', b'a', b'i', b'n']);
    }
}
