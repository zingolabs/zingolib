//! TODO: Add Mod Description Here!
use sapling_crypto::{
    bundle as sapling_bundle, circuit as sapling_circuit, keys as sapling_keys,
    prover as sapling_prover, value as sapling_value,
};
use std::path::PathBuf;
use zcash_primitives::transaction::TxId;
use zcash_proofs::prover;
use zcash_protocol::memo::MemoBytes;

pub use zl_file_format::strings::{read_string, write_string};

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
    /// - Parses the embedded Sapling parameters on this adapter's first call.
    fn prover(&self) -> &prover::LocalTxProver {
        self.prover.get_or_init(|| {
            #[cfg(test)]
            note_sapling_prover_parse();
            prover::LocalTxProver::from_bytes(
                crate::SaplingParams::SPEND,
                crate::SaplingParams::OUTPUT,
            )
        })
    }
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

    /// - Parses the embedded Sapling parameters if this adapter has not yet.
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

    /// - Parses the embedded Sapling parameters if this adapter has not yet.
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
mod tests;
