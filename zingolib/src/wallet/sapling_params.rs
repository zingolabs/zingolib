//! - Under the `runtime-sapling-params` feature, reads the Sapling proving
//!   parameters from the process-wide directory the consumer sets.

use std::borrow::Cow;
#[cfg(feature = "runtime-sapling-params")]
use std::path::{Path, PathBuf};
#[cfg(feature = "runtime-sapling-params")]
use std::sync::OnceLock;

use zcash_client_backend::proposal::Proposal;
use zcash_protocol::{PoolType, ShieldedPool};

// The published lengths and BLAKE2b digests of the Sapling MPC outputs, the
// values `zcash_proofs` verifies against and keeps private.
const SPEND_BYTES: usize = 47_958_396;
const OUTPUT_BYTES: usize = 3_592_860;
const SPEND_DIGEST: &str = "8270785a1a0d0bc77196f000ee6d221c9c9894f55307bd9357c3f0105d31ca63991ab91324160d8f53e2bbd3c2633a6eb8bdf5205d822e7f3f73edac51b2b70c";
const OUTPUT_DIGEST: &str = "657e3d38dbb5cb5e7dd2970e8b03d69b4787dd907285b5a7f0790dcc8072f60bf593b32cc2d1c030e00ff5ae64bf84c5c3beb84ddc841d48264b4a171744d028";

#[cfg(feature = "runtime-sapling-params")]
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaplingParamFile {
    Spend,
    Output,
}

impl SaplingParamFile {
    pub const ALL: [Self; 2] = [Self::Spend, Self::Output];

    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Spend => zcash_proofs::SAPLING_SPEND_NAME,
            Self::Output => zcash_proofs::SAPLING_OUTPUT_NAME,
        }
    }

    #[must_use]
    pub fn expected_len(self) -> usize {
        match self {
            Self::Spend => SPEND_BYTES,
            Self::Output => OUTPUT_BYTES,
        }
    }

    fn expected_digest(self) -> &'static str {
        match self {
            Self::Spend => SPEND_DIGEST,
            Self::Output => OUTPUT_DIGEST,
        }
    }

    pub fn verify(self, bytes: &[u8]) -> Result<(), SaplingParamsError> {
        if bytes.len() != self.expected_len() {
            return Err(SaplingParamsError::Length {
                file: self,
                found: bytes.len(),
            });
        }
        if blake2b_simd::State::new().update(bytes).finalize().to_hex().as_str() != self.expected_digest() {
            return Err(SaplingParamsError::Digest { file: self });
        }
        Ok(())
    }
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum SaplingParamsError {
    #[error("{} holds {found} bytes, not {}", .file.file_name(), .file.expected_len())]
    Length { file: SaplingParamFile, found: usize },
    #[error("{} does not match its published BLAKE2b digest", .file.file_name())]
    Digest { file: SaplingParamFile },
    #[cfg(feature = "runtime-sapling-params")]
    #[error("no Sapling parameter directory is set")]
    NoDirectory,
    #[cfg(feature = "runtime-sapling-params")]
    #[error("the Sapling parameter directory is set elsewhere, not to {}", .0.display())]
    DirectorySet(PathBuf),
    #[cfg(feature = "runtime-sapling-params")]
    #[error("reading {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

// A build makes Sapling proofs exactly when a step spends a Sapling note,
// pays a Sapling receiver, or takes Sapling change. The builder pads a Sapling
// bundle only once one of those has opened it.
#[must_use]
pub fn proves_sapling<FeeRule, NoteRef>(proposal: &Proposal<FeeRule, NoteRef>) -> bool {
    proposal.steps().iter().any(|step| {
        step.shielded_inputs().is_some_and(|inputs| {
            inputs
                .notes()
                .iter()
                .any(|note| note.note().pool() == ShieldedPool::Sapling)
        }) || step
            .payment_pools()
            .values()
            .any(|pool| *pool == PoolType::SAPLING)
            || step
                .balance()
                .proposed_change()
                .iter()
                .any(|change| change.output_pool() == PoolType::SAPLING)
    })
}

pub(crate) struct SaplingParamBytes {
    pub(crate) spend: Cow<'static, [u8]>,
    pub(crate) output: Cow<'static, [u8]>,
}

#[cfg(not(feature = "runtime-sapling-params"))]
pub(crate) fn load() -> Result<SaplingParamBytes, SaplingParamsError> {
    Ok(SaplingParamBytes {
        spend: Cow::Borrowed(crate::SaplingParams::SPEND),
        output: Cow::Borrowed(crate::SaplingParams::OUTPUT),
    })
}

/// - Sets the process-wide directory the parameters are read from. A second
///   call naming the same directory leaves it set.
#[cfg(feature = "runtime-sapling-params")]
pub fn set_directory(directory: PathBuf) -> Result<(), SaplingParamsError> {
    DIRECTORY.set(directory).or_else(|rejected| {
        DIRECTORY
            .get()
            .filter(|current| **current == rejected)
            .map(drop)
            .ok_or(SaplingParamsError::DirectorySet(rejected))
    })
}

/// - Reads both parameter files from the set directory.
#[cfg(feature = "runtime-sapling-params")]
pub(crate) fn load() -> Result<SaplingParamBytes, SaplingParamsError> {
    let directory = DIRECTORY.get().ok_or(SaplingParamsError::NoDirectory)?;
    Ok(SaplingParamBytes {
        spend: Cow::Owned(read(directory, SaplingParamFile::Spend)?),
        output: Cow::Owned(read(directory, SaplingParamFile::Output)?),
    })
}

/// - Reads `file` from `directory`.
#[cfg(feature = "runtime-sapling-params")]
fn read(directory: &Path, file: SaplingParamFile) -> Result<Vec<u8>, SaplingParamsError> {
    let path = directory.join(file.file_name());
    let bytes = std::fs::read(&path).map_err(|source| SaplingParamsError::Read { path, source })?;
    file.verify(&bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HYPOTHESIS: the parameters the build supplies pass the published
    /// length and digest checks. Falsified if either file fails `verify`.
    #[cfg(not(feature = "runtime-sapling-params"))]
    #[test]
    fn supplied_params_verify() {
        let params = load().expect("the embedded parameters are compiled in");
        SaplingParamFile::Spend
            .verify(&params.spend)
            .expect("the embedded spend parameters are the published file");
        SaplingParamFile::Output
            .verify(&params.output)
            .expect("the embedded output parameters are the published file");
    }

    /// HYPOTHESIS: the directory is set once, a repeat naming it is
    /// accepted, and a read from it reports the missing file by path.
    /// Falsified if another directory replaces it or the read succeeds.
    #[cfg(feature = "runtime-sapling-params")]
    #[test]
    fn directory_is_set_once() {
        const EMPTY_DIRECTORY: &str = "zingolib-sapling-params-unpopulated";
        const OTHER_DIRECTORY: &str = "zingolib-sapling-params-other";
        let directory = std::env::temp_dir().join(EMPTY_DIRECTORY);
        set_directory(directory.clone()).expect("no other test sets the directory");
        set_directory(directory).expect("a repeat names the same directory");
        assert!(matches!(
            set_directory(std::env::temp_dir().join(OTHER_DIRECTORY)),
            Err(SaplingParamsError::DirectorySet(_))
        ));
        assert!(matches!(load(), Err(SaplingParamsError::Read { .. })));
    }

    /// HYPOTHESIS: a file one byte short of the published length is refused
    /// before it is hashed. Falsified if `verify` accepts it or blames the digest.
    #[test]
    fn short_file_is_refused() {
        let short = vec![0; SaplingParamFile::Output.expected_len() - 1];
        assert!(matches!(
            SaplingParamFile::Output.verify(&short),
            Err(SaplingParamsError::Length { .. })
        ));
    }

    /// HYPOTHESIS: a file of the published length with other content is
    /// refused on its digest. Falsified if `verify` accepts it.
    #[test]
    fn substituted_file_is_refused() {
        let substituted = vec![0; SaplingParamFile::Output.expected_len()];
        assert!(matches!(
            SaplingParamFile::Output.verify(&substituted),
            Err(SaplingParamsError::Digest { .. })
        ));
    }
}
