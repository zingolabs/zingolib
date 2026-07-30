use orchard::note_encryption::CompactAction;
use sapling_crypto::{note::ExtractedNoteCommitment, note_encryption::CompactOutputDescription};
use zcash_note_encryption::EphemeralKeyBytes;
use zingo_netutils::lightwallet_protocol::{CompactOrchardAction, CompactSaplingOutput};

use crate::error::CompactFormatError;

pub(crate) mod block;
pub(crate) mod transaction;

pub(crate) fn get_compact_output_description(
    compact_sapling_output: &CompactSaplingOutput,
) -> Result<CompactOutputDescription, CompactFormatError> {
    let mut repr = [0; 32];
    repr.copy_from_slice(&compact_sapling_output.cmu[..]);
    let cmu = Option::from(ExtractedNoteCommitment::from_bytes(&repr))
        .ok_or(CompactFormatError::InvalidValue)?;

    let ephemeral_key = compact_sapling_output.ephemeral_key[..]
        .try_into()
        .map(EphemeralKeyBytes)
        .map_err(CompactFormatError::InvalidLength)?;

    Ok(CompactOutputDescription {
        cmu,
        ephemeral_key,
        enc_ciphertext: compact_sapling_output.ciphertext[..]
            .try_into()
            .map_err(CompactFormatError::InvalidLength)?,
    })
}

pub(crate) fn get_compact_action(
    compact_orchard_action: &CompactOrchardAction,
) -> Result<CompactAction, CompactFormatError> {
    let nf_bytes: [u8; 32] = compact_orchard_action.nullifier[..]
        .try_into()
        .map_err(CompactFormatError::InvalidLength)?;
    let nullifier = Option::from(orchard::note::Nullifier::from_bytes(&nf_bytes))
        .ok_or(CompactFormatError::InvalidValue)?;

    let cmx = Option::from(orchard::note::ExtractedNoteCommitment::from_bytes(
        &compact_orchard_action.cmx[..]
            .try_into()
            .map_err(CompactFormatError::InvalidLength)?,
    ))
    .ok_or(CompactFormatError::InvalidValue)?;

    let ephemeral_key = compact_orchard_action.ephemeral_key[..]
        .try_into()
        .map(EphemeralKeyBytes)
        .map_err(CompactFormatError::InvalidLength)?;

    Ok(CompactAction::from_parts(
        nullifier,
        cmx,
        ephemeral_key,
        compact_orchard_action.ciphertext[..]
            .try_into()
            .map_err(CompactFormatError::InvalidLength)?,
    ))
}
