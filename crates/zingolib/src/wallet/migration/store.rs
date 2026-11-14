//! Serialization of [`MigrationState`] into the wallet file.
//!
//! The section carries its own version byte, independent of
//! `LightWallet::serialized_version`, so future layout changes here never
//! bump the wallet version again.

use std::io::{self, Error, ErrorKind, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use zcash_encoding::{Optional, Vector};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use pepper_sync::wallet::OutputId;

use super::params::MigrationParams;
use super::parts::{BoundNote, BoundaryWitness, PartId, PartRecord, PartState, SigningStrategy};
use super::schedule::BucketGrid;
use super::{ConsentBinding, MigrationMode, MigrationPhase, MigrationState};
use crate::config::ChainType;

/// Version of this section's layout, bumped independently of the wallet
/// version. Version 2 appends the [`MigrationMode`] byte after the parts.
/// Version 3 drops the params `expiry_delta` field: the canonical expiry is
/// the fixed ZIP 318 formula, not a parameter. Version 4 appends each part's
/// `anchor_bucket` after its `bucket_index`, the anchor having become a
/// bucket of its own rather than the transmission window's (ADR 0018).
/// Version 5 numbers a part's buckets on the chain's ZIP 318 grid
/// ([`super::schedule::BucketGrid`]): a bucket index is the ordinal of a grid
/// boundary, which equals `height / M` below the NU7 activation and departs
/// from it above, where the grid's boundaries are the multiples of `3·M`.
/// The layout is unchanged; a section written before version 5 is renumbered
/// at read.
const INNER_VERSION: u8 = 5;

/// Serializes the migration section.
pub fn write<W: Write>(mut writer: W, state: &MigrationState) -> io::Result<()> {
    writer.write_u8(INNER_VERSION)?;

    let params = &state.params;
    writer.write_u32::<LittleEndian>(params.version)?;
    Vector::write(&mut writer, &params.denominations, |w, denomination| {
        w.write_u64::<LittleEndian>(*denomination)
    })?;
    writer.write_u64::<LittleEndian>(params.denom_cap)?;
    writer.write_u64::<LittleEndian>(params.max_residual_value)?;
    writer.write_u64::<LittleEndian>(params.sweep_min)?;
    writer.write_u32::<LittleEndian>(params.bucket_modulus)?;
    writer.write_u32::<LittleEndian>(params.k_max)?;
    writer.write_u32::<LittleEndian>(params.target_sessions)?;
    writer.write_u64::<LittleEndian>(params.max_actions_per_split_tx as u64)?;
    writer.write_u64::<LittleEndian>(params.part_fee)?;

    writer.write_all(&state.consent.params_hash)?;
    writer.write_all(&state.consent.plan_hash)?;
    writer.write_u64::<LittleEndian>(state.consent.consented_at)?;

    writer.write_u8(match state.strategy {
        SigningStrategy::LazyAtBoundary => 0,
        SigningStrategy::PreSigned => 1,
    })?;
    writer.write_u32::<LittleEndian>(state.account.into())?;

    match &state.phase {
        MigrationPhase::Planned => writer.write_u8(0)?,
        MigrationPhase::NoteSplitting {
            round,
            pending_txids,
        } => {
            writer.write_u8(1)?;
            writer.write_u32::<LittleEndian>(*round)?;
            Vector::write(&mut writer, pending_txids, |w, txid| txid.write(w))?;
        }
        MigrationPhase::PartsScheduled => writer.write_u8(2)?,
        MigrationPhase::Complete { residual } => {
            writer.write_u8(3)?;
            writer.write_u64::<LittleEndian>(*residual)?;
        }
    }

    Vector::write(&mut writer, &state.parts, |w, part| write_part(w, part))?;

    writer.write_u8(match state.mode {
        MigrationMode::Scheduled => 0,
        MigrationMode::Immediate => 1,
    })
}

/// Deserializes the migration section.
/// Reads the section. `chain` names the grid the parts' buckets are numbered
/// on, which a section written before version 5 is renumbered to.
pub fn read<R: Read>(mut reader: R, chain: ChainType) -> io::Result<MigrationState> {
    let inner_version = reader.read_u8()?;
    if inner_version > INNER_VERSION {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unknown migration section version {inner_version}"),
        ));
    }

    let version = reader.read_u32::<LittleEndian>()?;
    let denominations = Vector::read(&mut reader, |r| r.read_u64::<LittleEndian>())?;
    let denom_cap = reader.read_u64::<LittleEndian>()?;
    let max_residual_value = reader.read_u64::<LittleEndian>()?;
    let sweep_min = reader.read_u64::<LittleEndian>()?;
    let bucket_modulus = reader.read_u32::<LittleEndian>()?;
    // Every bucket computation divides or multiplies by the modulus, so a
    // zero from a corrupt file must fail typed here, not panic downstream.
    if bucket_modulus == 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "bucket_modulus must be nonzero",
        ));
    }
    let k_max = reader.read_u32::<LittleEndian>()?;
    let target_sessions = reader.read_u32::<LittleEndian>()?;
    let max_actions_per_split_tx =
        usize::try_from(reader.read_u64::<LittleEndian>()?).map_err(|_| {
            Error::new(
                ErrorKind::InvalidData,
                "max_actions_per_split_tx does not fit in this platform's usize",
            )
        })?;
    if inner_version <= 2 {
        // Discarded: superseded by the fixed ZIP 318 expiry formula.
        reader.read_u32::<LittleEndian>()?;
    }
    let part_fee = reader.read_u64::<LittleEndian>()?;
    let params = MigrationParams {
        version,
        denominations,
        denom_cap,
        max_residual_value,
        sweep_min,
        bucket_modulus,
        k_max,
        target_sessions,
        max_actions_per_split_tx,
        part_fee,
    };

    let mut params_hash = [0u8; 32];
    reader.read_exact(&mut params_hash)?;
    let mut plan_hash = [0u8; 32];
    reader.read_exact(&mut plan_hash)?;
    let consented_at = reader.read_u64::<LittleEndian>()?;
    let consent = ConsentBinding {
        params_hash,
        plan_hash,
        consented_at,
    };

    let strategy = match reader.read_u8()? {
        0 => SigningStrategy::LazyAtBoundary,
        1 => SigningStrategy::PreSigned,
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid signing strategy {other}"),
            ));
        }
    };
    let account = zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?)
        .map_err(|e| Error::new(ErrorKind::InvalidData, format!("invalid account id: {e}")))?;

    let phase = match reader.read_u8()? {
        0 => MigrationPhase::Planned,
        1 => {
            let round = reader.read_u32::<LittleEndian>()?;
            let pending_txids = Vector::read(&mut reader, |r| TxId::read(r))?;
            MigrationPhase::NoteSplitting {
                round,
                pending_txids,
            }
        }
        2 => MigrationPhase::PartsScheduled,
        3 => MigrationPhase::Complete {
            residual: reader.read_u64::<LittleEndian>()?,
        },
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid migration phase {other}"),
            ));
        }
    };

    let mut parts = Vector::read(&mut reader, |r| read_part(r, inner_version))?;
    if inner_version < 5 {
        rebase_uniform_bucket_indices(&mut parts, &params.grid(chain));
    }

    // Version 1 predates the mode marker. Scheduled is the conservative
    // reading: it makes the immediate path refuse to collapse the state
    // rather than assume the user consented to an immediate migration.
    let mode = if inner_version >= 2 {
        match reader.read_u8()? {
            0 => MigrationMode::Scheduled,
            1 => MigrationMode::Immediate,
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("invalid migration mode {other}"),
                ));
            }
        }
    } else {
        MigrationMode::Scheduled
    };

    Ok(MigrationState {
        params,
        consent,
        strategy,
        mode,
        account,
        phase,
        parts,
    })
}

/// Renumbers the buckets of a section written before version 5, when a
/// bucket index meant `height / M` on a uniform grid, onto the chain's grid,
/// where it is the ordinal of a grid boundary. The two agree below the NU7
/// activation. Above it a uniform boundary that is not a grid boundary opens
/// no bucket: such a transmission window moves up to the next grid boundary,
/// and such an anchor is dropped so the next placement draws a legal one,
/// with its cached witness unless a transaction already commits to it (as
/// the version 4 reading did for anchors drawn at age zero).
fn rebase_uniform_bucket_indices(parts: &mut [PartRecord], grid: &BucketGrid) {
    for part in parts {
        if let Some(index) = part.bucket_index {
            part.bucket_index = Some(
                grid.renumber_uniform_index(index)
                    .unwrap_or_else(|boundary| grid.index_at_or_after(boundary)),
            );
        }
        if let Some(index) = part.anchor_bucket {
            match grid.renumber_uniform_index(index) {
                Ok(index) => part.anchor_bucket = Some(index),
                Err(_) => {
                    part.anchor_bucket = None;
                    if !matches!(
                        part.state,
                        PartState::Signed | PartState::Broadcast | PartState::Confirmed { .. }
                    ) {
                        part.anchor_witness = None;
                    }
                }
            }
        }
    }
}

fn write_part<W: Write>(mut writer: W, part: &PartRecord) -> io::Result<()> {
    writer.write_u32::<LittleEndian>(part.id.0)?;
    writer.write_u64::<LittleEndian>(part.denomination)?;
    Optional::write(&mut writer, part.note.as_ref(), |mut w, note| {
        note.output_id.txid().write(&mut w)?;
        w.write_u32::<LittleEndian>(note.output_id.output_index())?;
        w.write_all(&note.nullifier)?;
        w.write_all(&note.commitment)
    })?;
    Optional::write(&mut writer, part.bucket_index, |w, bucket_index| {
        w.write_u64::<LittleEndian>(bucket_index)
    })?;
    Optional::write(&mut writer, part.anchor_bucket, |w, anchor_bucket| {
        w.write_u64::<LittleEndian>(anchor_bucket)
    })?;
    Optional::write(&mut writer, part.target_height, |w, target_height| {
        w.write_u32::<LittleEndian>(target_height.into())
    })?;
    match part.state {
        PartState::Bound => writer.write_u8(0)?,
        PartState::Assigned => writer.write_u8(1)?,
        PartState::Signed => writer.write_u8(2)?,
        PartState::Broadcast => writer.write_u8(3)?,
        PartState::Confirmed { height } => {
            writer.write_u8(4)?;
            writer.write_u32::<LittleEndian>(height.into())?;
        }
        PartState::Expired => writer.write_u8(5)?,
        PartState::Invalidated => writer.write_u8(6)?,
    }
    Optional::write(&mut writer, part.txid, |w, txid| txid.write(w))?;
    Optional::write(&mut writer, part.expiry_height, |w, expiry_height| {
        w.write_u32::<LittleEndian>(expiry_height.into())
    })?;
    Optional::write(&mut writer, part.signed_blob.as_ref(), |w, signed_blob| {
        Vector::write(w, signed_blob, |w, byte| w.write_u8(*byte))
    })?;
    Optional::write(
        &mut writer,
        part.anchor_witness.as_ref(),
        |mut w, witness| {
            w.write_all(&witness.anchor)?;
            w.write_u64::<LittleEndian>(witness.position)?;
            Vector::write(&mut w, &witness.auth_path, |w, node| w.write_all(node))
        },
    )?;
    writer.write_u8(part.attempts)
}

fn read_part<R: Read>(mut reader: R, inner_version: u8) -> io::Result<PartRecord> {
    let id = PartId(reader.read_u32::<LittleEndian>()?);
    let denomination = reader.read_u64::<LittleEndian>()?;
    let note = Optional::read(&mut reader, |mut r| {
        let txid = TxId::read(&mut r)?;
        let output_index = r.read_u32::<LittleEndian>()?;
        let mut nullifier = [0u8; 32];
        r.read_exact(&mut nullifier)?;
        let mut commitment = [0u8; 32];
        r.read_exact(&mut commitment)?;
        Ok(BoundNote {
            output_id: OutputId::new(txid, output_index),
            nullifier,
            commitment,
        })
    })?;
    let bucket_index = Optional::read(&mut reader, |r| r.read_u64::<LittleEndian>())?;
    let stored_anchor_bucket = if inner_version >= 4 {
        Optional::read(&mut reader, |r| r.read_u64::<LittleEndian>())?
    } else {
        None
    };
    let target_height = if inner_version >= 1 {
        Optional::read(&mut reader, |r| {
            Ok(BlockHeight::from_u32(r.read_u32::<LittleEndian>()?))
        })?
    } else {
        None
    };
    let state = match reader.read_u8()? {
        0 => PartState::Bound,
        1 => PartState::Assigned,
        2 => PartState::Signed,
        3 => PartState::Broadcast,
        4 => PartState::Confirmed {
            height: BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?),
        },
        5 => PartState::Expired,
        6 => PartState::Invalidated,
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid part state {other}"),
            ));
        }
    };
    let txid = Optional::read(&mut reader, TxId::read)?;
    let expiry_height = Optional::read(&mut reader, |r| {
        Ok(BlockHeight::from_u32(r.read_u32::<LittleEndian>()?))
    })?;
    let signed_blob = Optional::read(&mut reader, |r| {
        Vector::read(r, byteorder::ReadBytesExt::read_u8)
    })?;
    let anchor_witness = Optional::read(&mut reader, |mut r| {
        let mut anchor = [0u8; 32];
        r.read_exact(&mut anchor)?;
        let position = r.read_u64::<LittleEndian>()?;
        let auth_path = Vector::read(&mut r, |r| {
            let mut node = [0u8; 32];
            r.read_exact(&mut node)?;
            Ok(node)
        })?;
        Ok(BoundaryWitness {
            anchor,
            position,
            auth_path,
        })
    })?;
    let attempts = reader.read_u8()?;

    // Before version 4 the anchor *was* the transmission window's boundary, an
    // age of zero. A part that already carries a transaction committed to
    // that anchor keeps it: the signature cannot be re-aimed, so recording
    // anything else would misdescribe what is on the wire. An unsigned part
    // has committed to nothing, so it is left anchorless and the next
    // placement (or `refresh_part_witnesses`) draws it a legal age (ADR 0018).
    //
    // Its cached witness goes with it. That witness proves the note under the
    // *window's* boundary, which is not where the part will anchor once an
    // age is drawn, so keeping it would either fail to prove or quietly prove
    // against the age-zero anchor this change exists to retire.
    let (anchor_bucket, anchor_witness) = if inner_version >= 4 {
        (stored_anchor_bucket, anchor_witness)
    } else {
        match state {
            PartState::Signed | PartState::Broadcast | PartState::Confirmed { .. } => {
                (bucket_index, anchor_witness)
            }
            PartState::Bound
            | PartState::Assigned
            | PartState::Expired
            | PartState::Invalidated => (None, None),
        }
    };

    Ok(PartRecord {
        id,
        denomination,
        note,
        bucket_index,
        anchor_bucket,
        target_height,
        state,
        txid,
        expiry_height,
        signed_blob,
        anchor_witness,
        attempts,
    })
}

#[cfg(test)]
mod tests;
