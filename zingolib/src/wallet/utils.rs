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

use super::sapling_params::{self, SaplingParamBytes, SaplingParamsError};

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
    static SAPLING_PROVER_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn sapling_prover_parses() -> usize {
    SAPLING_PROVER_PARSES.with(std::cell::Cell::get)
}

/// - Increments this thread's count of Sapling parameter parses.
#[cfg(test)]
fn note_sapling_prover_parse() {
    SAPLING_PROVER_PARSES.with(|parses| parses.set(parses.get() + 1));
}

#[derive(Default)]
pub(crate) struct LazySaplingProver {
    prover: std::sync::OnceLock<prover::LocalTxProver>,
}

impl LazySaplingProver {
    /// - Loads and parses the Sapling parameters now, for a build that
    ///   makes Sapling proofs.
    pub(crate) fn parsed() -> Result<Self, SaplingParamsError> {
        Ok(Self {
            prover: std::sync::OnceLock::from(parse_sapling_prover(&sapling_params::load()?)),
        })
    }

    /// - Loads and parses the Sapling parameters on this adapter's first call.
    /// - Panics when the parameters cannot be loaded.
    fn prover(&self) -> &prover::LocalTxProver {
        self.prover.get_or_init(|| {
            parse_sapling_prover(&sapling_params::load().expect(
                "a build that makes Sapling proofs takes `LazySaplingProver::parsed`, \
                 and embedded parameters always load",
            ))
        })
    }
}

/// - Increments this thread's count of Sapling parameter parses, in tests.
fn parse_sapling_prover(params: &SaplingParamBytes) -> prover::LocalTxProver {
    #[cfg(test)]
    note_sapling_prover_parse();
    prover::LocalTxProver::from_bytes(&params.spend, &params.output)
}

impl sapling_prover::SpendProver for LazySaplingProver {
    type Proof = <prover::LocalTxProver as sapling_prover::SpendProver>::Proof;

    fn prepare_circuit(
        proof_generation_key: sapling_crypto::ProofGenerationKey,
        diversifier: sapling_crypto::Diversifier,
        rseed: sapling_crypto::Rseed,
        value: sapling_value::NoteValue,
        alpha: jubjub::Fr,
        rcv: sapling_value::ValueCommitTrapdoor,
        anchor: jubjub::Fq,
        merkle_path: sapling_crypto::MerklePath,
    ) -> Option<sapling_circuit::Spend> {
        <prover::LocalTxProver as sapling_prover::SpendProver>::prepare_circuit(
            proof_generation_key,
            diversifier,
            rseed,
            value,
            alpha,
            rcv,
            anchor,
            merkle_path,
        )
    }

    /// - Loads and parses the Sapling parameters if this adapter has not yet.
    fn create_proof<R: rand::Rng>(
        &self,
        circuit: sapling_circuit::Spend,
        rng: &mut R,
    ) -> Self::Proof {
        sapling_prover::SpendProver::create_proof(self.prover(), circuit, rng)
    }

    fn encode_proof(proof: Self::Proof) -> sapling_bundle::GrothProofBytes {
        <prover::LocalTxProver as sapling_prover::SpendProver>::encode_proof(proof)
    }
}

impl sapling_prover::OutputProver for LazySaplingProver {
    type Proof = <prover::LocalTxProver as sapling_prover::OutputProver>::Proof;

    fn prepare_circuit(
        esk: &sapling_keys::EphemeralSecretKey,
        payment_address: sapling_crypto::PaymentAddress,
        rcm: jubjub::Fr,
        value: sapling_value::NoteValue,
        rcv: sapling_value::ValueCommitTrapdoor,
    ) -> sapling_circuit::Output {
        <prover::LocalTxProver as sapling_prover::OutputProver>::prepare_circuit(
            esk,
            payment_address,
            rcm,
            value,
            rcv,
        )
    }

    /// - Loads and parses the Sapling parameters if this adapter has not yet.
    fn create_proof<R: rand::Rng>(
        &self,
        circuit: sapling_circuit::Output,
        rng: &mut R,
    ) -> Self::Proof {
        sapling_prover::OutputProver::create_proof(self.prover(), circuit, rng)
    }

    fn encode_proof(proof: Self::Proof) -> sapling_bundle::GrothProofBytes {
        <prover::LocalTxProver as sapling_prover::OutputProver>::encode_proof(proof)
    }
}

/// Returns the path to the default directory that the Zcash proving parameters are located in.
pub fn get_zcash_params_path() -> std::io::Result<PathBuf> {
    zcash_proofs::default_params_folder()
        .ok_or_else(|| std::io::Error::other("could not load default params folder"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
