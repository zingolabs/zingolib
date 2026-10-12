//! TODO: Add Mod Description Here!
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use sapling_crypto::{
    bundle as sapling_bundle, circuit as sapling_circuit, keys as sapling_keys,
    prover as sapling_prover, value as sapling_value,
};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};
use zcash_primitives::transaction::TxId;
use zcash_proofs::prover;
use zcash_protocol::memo::MemoBytes;

pub(crate) const MAX_STRING_LENGTH: u64 = 1 << 16;

/// ```
/// use zingolib::wallet::utils::{read_string, write_string};
///
/// let mut encoded = Vec::new();
/// write_string(&mut encoded, &"main".to_string()).unwrap();
/// assert_eq!(read_string(encoded.as_slice()).unwrap(), "main");
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

    String::from_utf8(str_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// TODO: Add Doc Comment Here!
pub fn write_string<W: Write>(mut writer: W, s: &String) -> io::Result<()> {
    // Strings are written as len + utf8
    writer.write_u64::<LittleEndian>(s.len() as u64)?;
    writer.write_all(s.as_bytes())
}

/// Failure to interpret a memo string.
///
/// Carries the offending memo's byte length, never its content, so the
/// memo cannot leak into logs or error channels.
#[derive(Debug, thiserror::Error)]
pub enum MemoError {
    /// The encoded memo exceeds the memo field's capacity.
    #[error("Error creating output. Memo of {0} bytes is too long")]
    TooLong(usize),
}

/// Create memo bytes from string.
pub fn memo_bytes_from_string(memo_str: String) -> Result<MemoBytes, MemoError> {
    let s_bytes = Vec::from(memo_str.as_bytes());
    MemoBytes::from_bytes(&s_bytes).map_err(|_| MemoError::TooLong(s_bytes.len()))
}

/// TODO: Add Doc Comment Here!
#[must_use]
pub fn txid_from_slice(txid: &[u8]) -> TxId {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(txid);
    TxId::from_bytes(txid_bytes)
}

#[cfg(test)]
thread_local! {
    static SAPLING_PROVER_REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn sapling_prover_requests() -> usize {
    SAPLING_PROVER_REQUESTS.with(std::cell::Cell::get)
}

/// - Increments this thread's count of Sapling prover requests.
#[cfg(test)]
fn note_sapling_prover_request() {
    SAPLING_PROVER_REQUESTS.with(|requests| requests.set(requests.get() + 1));
}

static SAPLING_PROVER: std::sync::OnceLock<prover::LocalTxProver> = std::sync::OnceLock::new();

/// - Parses the embedded Sapling parameters on the first call and holds the
///   result in a process-global for the life of the process.
fn embedded_prover() -> &'static prover::LocalTxProver {
    #[cfg(test)]
    note_sapling_prover_request();
    SAPLING_PROVER.get_or_init(|| {
        prover::LocalTxProver::from_bytes(crate::SaplingParams::SPEND, crate::SaplingParams::OUTPUT)
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LazySaplingProver;

macro_rules! delegate_sapling_prover {
    (
        $prover_trait:ident,
        $circuit:ty,
        fn prepare_circuit($($arg:ident: $arg_ty:ty),* $(,)?) -> $prepared:ty
    ) => {
        impl sapling_prover::$prover_trait for LazySaplingProver {
            type Proof = <prover::LocalTxProver as sapling_prover::$prover_trait>::Proof;

            fn prepare_circuit($($arg: $arg_ty),*) -> $prepared {
                <prover::LocalTxProver as sapling_prover::$prover_trait>::prepare_circuit(
                    $($arg),*
                )
            }

            /// - Parses the embedded Sapling parameters if no earlier Sapling proof
            ///   in this process has done so.
            fn create_proof<R: rand::Rng>(&self, circuit: $circuit, rng: &mut R) -> Self::Proof {
                sapling_prover::$prover_trait::create_proof(embedded_prover(), circuit, rng)
            }

            fn encode_proof(proof: Self::Proof) -> sapling_bundle::GrothProofBytes {
                <prover::LocalTxProver as sapling_prover::$prover_trait>::encode_proof(proof)
            }
        }
    };
}

delegate_sapling_prover!(
    SpendProver,
    sapling_circuit::Spend,
    fn prepare_circuit(
        proof_generation_key: sapling_crypto::ProofGenerationKey,
        diversifier: sapling_crypto::Diversifier,
        rseed: sapling_crypto::Rseed,
        value: sapling_value::NoteValue,
        alpha: jubjub::Fr,
        rcv: sapling_value::ValueCommitTrapdoor,
        anchor: jubjub::Fq,
        merkle_path: sapling_crypto::MerklePath,
    ) -> Option<sapling_circuit::Spend>
);

delegate_sapling_prover!(
    OutputProver,
    sapling_circuit::Output,
    fn prepare_circuit(
        esk: &sapling_keys::EphemeralSecretKey,
        payment_address: sapling_crypto::PaymentAddress,
        rcm: jubjub::Fr,
        value: sapling_value::NoteValue,
        rcv: sapling_value::ValueCommitTrapdoor,
    ) -> sapling_circuit::Output
);

/// Returns the path to the default directory that the Zcash proving parameters are located in.
pub fn get_zcash_params_path() -> std::io::Result<PathBuf> {
    zcash_proofs::default_params_folder()
        .ok_or_else(|| std::io::Error::other("could not load default params folder"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HYPOTHESIS: the embedded Sapling parameters are present whole, so
    /// loading them is infallible. Falsified if either blob is empty.
    #[test]
    fn embedded_sapling_params_load_whole() {
        assert!(
            !crate::SaplingParams::OUTPUT.is_empty(),
            "output params are embedded"
        );
        assert!(
            !crate::SaplingParams::SPEND.is_empty(),
            "spend params are embedded"
        );
    }

    /// HYPOTHESIS: holding the adapter costs nothing, so binding it at a
    /// build site cannot trigger the parse. Falsified if it carries state.
    #[test]
    fn lazy_sapling_prover_is_zero_sized() {
        assert_eq!(std::mem::size_of::<LazySaplingProver>(), 0);
    }

    /// HYPOTHESIS: the Sapling prover is parsed once per process. Falsified
    /// if two calls hand back different allocations.
    #[test]
    fn embedded_prover_is_parsed_once() {
        assert!(std::ptr::eq(embedded_prover(), embedded_prover()));
    }

    #[test]
    fn test_memo_bytes_from_string_plain_text() {
        let result = memo_bytes_from_string("Hello World".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_memo_bytes_from_string_valid_hex() {
        // Valid hex should be decoded
        let result = memo_bytes_from_string("0x48656c6c6f".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_memo_bytes_from_string_ethereum_address() {
        // Ethereum address should be treated as plain text
        let eth_address = "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb";
        let result = memo_bytes_from_string(eth_address.to_string());
        assert!(result.is_ok());
        // Verify it's stored as the full string (43 bytes), not decoded hex (20 bytes)
        let memo_bytes = result.unwrap();
        assert_eq!(memo_bytes.as_slice().len(), eth_address.len());
    }

    #[test]
    fn test_memo_bytes_from_string_invalid_hex() {
        // Invalid hex should fall back to plain text
        let result = memo_bytes_from_string("0xZZZZ".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_memo_bytes_from_string_too_long() {
        // String longer than 512 bytes should fail
        let long_string = "a".repeat(513);
        let result = memo_bytes_from_string(long_string);
        assert!(result.is_err());
    }
}
