//! Snapshot witness validation, shared by the session before any proving.
//!
//! zingolib produces the [`WitnessData`] rows from the wallet's orchard shard
//! tree at the snapshot height (see zingolib's `lightclient::voting` module).
//! This module checks the set the session receives: one witness per note, at
//! the note's position, with the note's commitment, rooted at the round's
//! authenticated `nc_root`. A witness that fails here stops the lifecycle
//! before any proof material is generated.

use crate::error::VotingFailure;
use zcash_voting::types::{NoteInfo, WitnessData};
use zcash_voting::wire::VotingRoundParams;

/// Validates `witnesses` against `note_infos` and the round's params.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the counts or positions disagree, and
/// [`VotingFailure::WitnessRootMismatch`] when any witness roots at a value
/// other than the round's authenticated `nc_root`.
pub fn validate_witnesses(
    witnesses: &[WitnessData],
    note_infos: &[NoteInfo],
    params: &VotingRoundParams,
) -> Result<(), VotingFailure> {
    if witnesses.len() != note_infos.len() {
        return Err(VotingFailure::Sdk(
            zcash_voting::VotingError::InvalidInput {
                message: format!(
                    "{} witnesses for {} snapshot notes",
                    witnesses.len(),
                    note_infos.len()
                ),
            },
        ));
    }
    for (witness, note) in witnesses.iter().zip(note_infos) {
        if witness.position != note.position {
            return Err(VotingFailure::Sdk(
                zcash_voting::VotingError::InvalidInput {
                    message: format!(
                        "witness position {} does not match note position {}",
                        witness.position, note.position
                    ),
                },
            ));
        }
        if witness.note_commitment != note.commitment {
            return Err(VotingFailure::Sdk(
                zcash_voting::VotingError::InvalidInput {
                    message: format!(
                        "witness commitment does not match note at position {}",
                        note.position
                    ),
                },
            ));
        }
        if witness.root != params.nc_root {
            return Err(VotingFailure::WitnessRootMismatch {
                height: params.snapshot_height,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> VotingRoundParams {
        VotingRoundParams {
            vote_round_id: "round".to_string(),
            snapshot_height: 100,
            ea_pk: vec![1u8; 32],
            nc_root: vec![7u8; 32],
            nullifier_imt_root: vec![8u8; 32],
        }
    }

    fn note(position: u64) -> NoteInfo {
        NoteInfo {
            commitment: vec![3u8; 32],
            nullifier: vec![4u8; 32],
            value: 1_000,
            position,
            diversifier: vec![5u8; 11],
            rho: vec![6u8; 32],
            rseed: vec![9u8; 32],
            scope: 0,
            ufvk_str: "uview".to_string(),
        }
    }

    fn witness(position: u64, root: Vec<u8>) -> WitnessData {
        WitnessData {
            note_commitment: vec![3u8; 32],
            position,
            root,
            auth_path: vec![vec![0u8; 32]; 32],
        }
    }

    #[test]
    fn a_matching_set_validates() {
        validate_witnesses(&[witness(5, vec![7u8; 32])], &[note(5)], &params())
            .expect("matching witness set validates");
    }

    #[test]
    fn a_witness_count_mismatch_is_refused() {
        let error = validate_witnesses(&[], &[note(5)], &params())
            .expect_err("zero witnesses for one note");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn a_position_mismatch_is_refused() {
        let error = validate_witnesses(&[witness(6, vec![7u8; 32])], &[note(5)], &params())
            .expect_err("witness at position 6 for a note at 5");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn a_commitment_mismatch_is_refused() {
        let mut wrong = witness(5, vec![7u8; 32]);
        wrong.note_commitment = vec![0u8; 32];
        let error =
            validate_witnesses(&[wrong], &[note(5)], &params()).expect_err("foreign commitment");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn a_root_outside_the_authenticated_nc_root_is_refused() {
        let error = validate_witnesses(&[witness(5, vec![9u8; 32])], &[note(5)], &params())
            .expect_err("witness rooted at a foreign value");
        assert!(matches!(
            error,
            VotingFailure::WitnessRootMismatch { height: 100 }
        ));
    }
}
