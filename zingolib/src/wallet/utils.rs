//! TODO: Add Mod Description Here!
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{self, Read, Write};
use zcash_primitives::{memo::MemoBytes, transaction::TxId};

use super::data::WalletZecPriceInfo;

/// TODO: Add Doc Comment Here!
pub fn read_string<R: Read>(mut reader: R) -> io::Result<String> {
    // Strings are written as <littleendian> len + bytes
    let str_len = reader.read_u64::<LittleEndian>()?;
    let mut str_bytes = vec![0; str_len as usize];
    reader.read_exact(&mut str_bytes)?;

    let str = String::from_utf8(str_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(str)
}

/// TODO: Add Doc Comment Here!
pub fn write_string<W: Write>(mut writer: W, s: &String) -> io::Result<()> {
    // Strings are written as len + utf8
    writer.write_u64::<LittleEndian>(s.as_bytes().len() as u64)?;
    writer.write_all(s.as_bytes())
}

/// Interpret a string or hex-encoded memo, and return a Memo object
pub fn interpret_memo_string(memo_str: String) -> Result<MemoBytes, String> {
    // If the string starts with an "0x", and contains only hex chars ([a-f0-9]+) then
    // interpret it as a hex
    let s_bytes = if memo_str.to_lowercase().starts_with("0x") {
        match hex::decode(&memo_str[2..memo_str.len()]) {
            Ok(data) => data,
            Err(_) => Vec::from(memo_str.as_bytes()),
        }
    } else {
        Vec::from(memo_str.as_bytes())
    };

    MemoBytes::from_bytes(&s_bytes)
        .map_err(|_| format!("Error creating output. Memo '{:?}' is too long", memo_str))
}

/// TODO: Add Doc Comment Here!
pub fn txid_from_slice(txid: &[u8]) -> TxId {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(txid);
    TxId::from_bytes(txid_bytes)
}

/// TODO: Add Doc Comment Here!
pub fn get_price(datetime: u64, price: &WalletZecPriceInfo) -> Option<f64> {
    match price.zec_price {
        None => None,
        Some((t, p)) => {
            // If the price was fetched within 24 hours of this Tx, we use the "current" price
            // else, we mark it as None, for the historical price fetcher to get
            // TODO:  Investigate the state of "the historical price fetcher".
            if (t as i64 - datetime as i64).abs() < 24 * 60 * 60 {
                Some(p)
            } else {
                None
            }
        }
    }
}
pub(crate) fn read_sapling_params() -> Result<(Vec<u8>, Vec<u8>), String> {
    use crate::SaplingParams;
    let mut sapling_output = vec![];
    sapling_output.extend_from_slice(
        SaplingParams::get("sapling-output.params")
            .unwrap()
            .data
            .as_ref(),
    );

    let mut sapling_spend = vec![];
    sapling_spend.extend_from_slice(
        SaplingParams::get("sapling-spend.params")
            .unwrap()
            .data
            .as_ref(),
    );

    Ok((sapling_output, sapling_spend))
}
