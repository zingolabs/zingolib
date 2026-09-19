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
use super::transfers::{
    BoundNote, BoundaryWitness, SigningStrategy, TransferId, TransferRecord, TransferState,
};
use super::{MigrationMode, MigrationPhase, MigrationState, PlanCommitment};

/// Version of this section's layout, bumped independently of the wallet
/// version. Version 2 appends the [`MigrationMode`] byte after the transfers.
/// Version 3 drops the params `expiry_delta` field: the canonical expiry is
/// the fixed ZIP 318 formula, not a parameter. Version 4 appends each transfer's
/// `anchor_bucket` after its `bucket_index`, the anchor having become a
/// bucket of its own rather than the broadcast window's (ADR 0018).
/// Version 5 appends each transfer's `previous_txids` and `missed_windows`
/// after its `attempts`, adds the `Prepared` phase (code 4) and the
/// `Released` transfer state (code 7).
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
    writer.write_u64::<LittleEndian>(params.transfer_fee)?;

    writer.write_all(&state.commitment.params_hash)?;
    writer.write_all(&state.commitment.plan_hash)?;
    writer.write_u64::<LittleEndian>(state.commitment.committed_at)?;

    writer.write_u8(match state.strategy {
        SigningStrategy::LazyAtBoundary => 0,
        SigningStrategy::PreSigned => 1,
    })?;
    writer.write_u32::<LittleEndian>(state.account.into())?;

    match &state.phase {
        MigrationPhase::Committed => writer.write_u8(0)?,
        MigrationPhase::Preparing {
            round,
            pending_txids,
        } => {
            writer.write_u8(1)?;
            writer.write_u32::<LittleEndian>(*round)?;
            Vector::write(&mut writer, pending_txids, |w, txid| txid.write(w))?;
        }
        MigrationPhase::Scheduled => writer.write_u8(2)?,
        MigrationPhase::Complete { residual } => {
            writer.write_u8(3)?;
            writer.write_u64::<LittleEndian>(*residual)?;
        }
        MigrationPhase::Prepared => writer.write_u8(4)?,
    }

    Vector::write(&mut writer, &state.transfers, |w, transfer| {
        write_part(w, transfer)
    })?;

    writer.write_u8(match state.mode {
        MigrationMode::Scheduled => 0,
        MigrationMode::Immediate => 1,
    })
}

/// Deserializes the migration section.
pub fn read<R: Read>(mut reader: R) -> io::Result<MigrationState> {
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
    let transfer_fee = reader.read_u64::<LittleEndian>()?;
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
        transfer_fee,
    };
    params
        .validate()
        .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;

    let mut params_hash = [0u8; 32];
    reader.read_exact(&mut params_hash)?;
    let mut plan_hash = [0u8; 32];
    reader.read_exact(&mut plan_hash)?;
    let committed_at = reader.read_u64::<LittleEndian>()?;
    let commitment = PlanCommitment {
        params_hash,
        plan_hash,
        committed_at,
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
        0 => MigrationPhase::Committed,
        1 => {
            let round = reader.read_u32::<LittleEndian>()?;
            let pending_txids = Vector::read(&mut reader, |r| TxId::read(r))?;
            MigrationPhase::Preparing {
                round,
                pending_txids,
            }
        }
        2 => MigrationPhase::Scheduled,
        3 => MigrationPhase::Complete {
            residual: reader.read_u64::<LittleEndian>()?,
        },
        4 => MigrationPhase::Prepared,
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid migration phase {other}"),
            ));
        }
    };

    let transfers = Vector::read(&mut reader, |r| read_part(r, inner_version))?;
    for transfer in &transfers {
        for bucket in [transfer.bucket_index, transfer.anchor_bucket]
            .into_iter()
            .flatten()
        {
            if !bucket_fits_block_height(bucket, bucket_modulus) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("bucket index {bucket} exceeds the block height range"),
                ));
            }
        }
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
        commitment,
        strategy,
        mode,
        account,
        phase,
        transfers,
    })
}

fn bucket_fits_block_height(bucket: u64, bucket_modulus: u32) -> bool {
    bucket
        .checked_add(1)
        .and_then(|next| next.checked_mul(u64::from(bucket_modulus)))
        .is_some_and(|window_end| u32::try_from(window_end).is_ok())
}

fn write_part<W: Write>(mut writer: W, transfer: &TransferRecord) -> io::Result<()> {
    writer.write_u32::<LittleEndian>(transfer.id.0)?;
    writer.write_u64::<LittleEndian>(transfer.denomination)?;
    Optional::write(&mut writer, transfer.note.as_ref(), |mut w, note| {
        note.output_id.txid().write(&mut w)?;
        w.write_u32::<LittleEndian>(note.output_id.output_index())?;
        w.write_all(&note.nullifier)?;
        w.write_all(&note.commitment)
    })?;
    Optional::write(&mut writer, transfer.bucket_index, |w, bucket_index| {
        w.write_u64::<LittleEndian>(bucket_index)
    })?;
    Optional::write(&mut writer, transfer.anchor_bucket, |w, anchor_bucket| {
        w.write_u64::<LittleEndian>(anchor_bucket)
    })?;
    Optional::write(&mut writer, transfer.target_height, |w, target_height| {
        w.write_u32::<LittleEndian>(target_height.into())
    })?;
    match transfer.state {
        TransferState::Bound => writer.write_u8(0)?,
        TransferState::Assigned => writer.write_u8(1)?,
        TransferState::Signed => writer.write_u8(2)?,
        TransferState::Broadcast => writer.write_u8(3)?,
        TransferState::Confirmed { height } => {
            writer.write_u8(4)?;
            writer.write_u32::<LittleEndian>(height.into())?;
        }
        TransferState::Expired => writer.write_u8(5)?,
        TransferState::Invalidated => writer.write_u8(6)?,
        TransferState::Released => writer.write_u8(7)?,
    }
    Optional::write(&mut writer, transfer.txid, |w, txid| txid.write(w))?;
    Optional::write(&mut writer, transfer.expiry_height, |w, expiry_height| {
        w.write_u32::<LittleEndian>(expiry_height.into())
    })?;
    Optional::write(
        &mut writer,
        transfer.signed_blob.as_ref(),
        |w, signed_blob| Vector::write(w, signed_blob, |w, byte| w.write_u8(*byte)),
    )?;
    Optional::write(
        &mut writer,
        transfer.anchor_witness.as_ref(),
        |mut w, witness| {
            w.write_all(&witness.anchor)?;
            w.write_u64::<LittleEndian>(witness.position)?;
            Vector::write(&mut w, &witness.auth_path, |w, node| w.write_all(node))
        },
    )?;
    writer.write_u8(transfer.attempts)?;
    Vector::write(&mut writer, &transfer.previous_txids, |w, txid| {
        txid.write(w)
    })?;
    writer.write_u32::<LittleEndian>(transfer.missed_windows)
}

fn read_part<R: Read>(mut reader: R, inner_version: u8) -> io::Result<TransferRecord> {
    let id = TransferId(reader.read_u32::<LittleEndian>()?);
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
        0 => TransferState::Bound,
        1 => TransferState::Assigned,
        2 => TransferState::Signed,
        3 => TransferState::Broadcast,
        4 => TransferState::Confirmed {
            height: BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?),
        },
        5 => TransferState::Expired,
        6 => TransferState::Invalidated,
        7 => TransferState::Released,
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid transfer state {other}"),
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
    let (previous_txids, missed_windows) = if inner_version >= 5 {
        (
            Vector::read(&mut reader, |r| TxId::read(r))?,
            reader.read_u32::<LittleEndian>()?,
        )
    } else {
        (Vec::new(), 0)
    };

    // Before version 4 the anchor *was* the broadcast window's boundary, an
    // age of zero. A transfer that already carries a transaction committed to
    // that anchor keeps it: the signature cannot be re-aimed, so recording
    // anything else would misdescribe what is on the wire. An unsigned transfer
    // has committed to nothing, so it is left anchorless and the next
    // placement (or `refresh_transfer_witnesses`) draws it a legal age (ADR 0018).
    //
    // Its cached witness goes with it. That witness proves the note under the
    // *window's* boundary, which is not where the transfer will anchor once an
    // age is drawn, so keeping it would either fail to prove or quietly prove
    // against the age-zero anchor this change exists to retire.
    let (anchor_bucket, anchor_witness) = if inner_version >= 4 {
        (stored_anchor_bucket, anchor_witness)
    } else {
        match state {
            TransferState::Signed | TransferState::Broadcast | TransferState::Confirmed { .. } => {
                (bucket_index, anchor_witness)
            }
            TransferState::Bound
            | TransferState::Assigned
            | TransferState::Expired
            | TransferState::Invalidated
            | TransferState::Released => (None, None),
        }
    };

    Ok(TransferRecord {
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
        previous_txids,
        missed_windows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arbitrary_params() -> impl Strategy<Value = MigrationParams> {
        (
            any::<u32>(),
            any::<u64>(),
            any::<u64>(),
            0u64..=1_000_000,
            1u32..,
            1u32..,
            1u32..,
            2usize..=1000,
            any::<u64>(),
        )
            .prop_flat_map(
                |(
                    version,
                    denom_cap,
                    max_residual_value,
                    sweep_min,
                    bucket_modulus,
                    k_max,
                    target_sessions,
                    max_actions_per_split_tx,
                    transfer_fee,
                )| {
                    proptest::collection::btree_set(sweep_min + 1..=10_000_000_000u64, 1..8)
                        .prop_map(move |denominations| MigrationParams {
                            version,
                            denominations: denominations.into_iter().rev().collect(),
                            denom_cap,
                            max_residual_value,
                            sweep_min,
                            bucket_modulus,
                            k_max,
                            target_sessions,
                            max_actions_per_split_tx,
                            transfer_fee,
                        })
                },
            )
    }

    fn arbitrary_txid() -> impl Strategy<Value = TxId> {
        any::<[u8; 32]>().prop_map(TxId::from_bytes)
    }

    fn arbitrary_state() -> impl Strategy<Value = TransferState> {
        prop_oneof![
            Just(TransferState::Bound),
            Just(TransferState::Assigned),
            Just(TransferState::Signed),
            Just(TransferState::Broadcast),
            any::<u32>().prop_map(|h| TransferState::Confirmed {
                height: BlockHeight::from_u32(h)
            }),
            Just(TransferState::Expired),
            Just(TransferState::Invalidated),
            Just(TransferState::Released),
        ]
    }

    fn arbitrary_part() -> impl Strategy<Value = TransferRecord> {
        (
            (
                any::<u32>(),
                any::<u64>(),
                proptest::option::of((
                    arbitrary_txid(),
                    any::<u32>(),
                    any::<[u8; 32]>(),
                    any::<[u8; 32]>(),
                )),
                proptest::option::of(any::<u64>()),
                proptest::option::of(any::<u32>()),
                arbitrary_state(),
            ),
            (
                proptest::option::of(arbitrary_txid()),
                proptest::option::of(any::<u32>()),
                proptest::option::of(proptest::collection::vec(any::<u8>(), 0..64)),
                proptest::option::of((
                    any::<[u8; 32]>(),
                    any::<u64>(),
                    proptest::collection::vec(any::<[u8; 32]>(), 0..32),
                )),
                any::<u8>(),
                proptest::option::of(any::<u64>()),
            ),
            (
                proptest::collection::vec(arbitrary_txid(), 0..3),
                any::<u32>(),
            ),
        )
            .prop_map(
                |(
                    (id, denomination, note, bucket_index, target_height, state),
                    (txid, expiry_height, signed_blob, anchor_witness, attempts, anchor_bucket),
                    (previous_txids, missed_windows),
                )| TransferRecord {
                    id: TransferId(id),
                    denomination,
                    note: note.map(|(txid, output_index, nullifier, commitment)| BoundNote {
                        output_id: OutputId::new(txid, output_index),
                        nullifier,
                        commitment,
                    }),
                    bucket_index,
                    anchor_bucket,
                    target_height: target_height.map(BlockHeight::from_u32),
                    state,
                    txid,
                    expiry_height: expiry_height.map(BlockHeight::from_u32),
                    signed_blob,
                    anchor_witness: anchor_witness.map(|(anchor, position, auth_path)| {
                        BoundaryWitness {
                            anchor,
                            position,
                            auth_path,
                        }
                    }),
                    attempts,
                    previous_txids,
                    missed_windows,
                },
            )
    }

    fn arbitrary_phase() -> impl Strategy<Value = MigrationPhase> {
        prop_oneof![
            Just(MigrationPhase::Committed),
            (
                any::<u32>(),
                proptest::collection::vec(arbitrary_txid(), 0..5)
            )
                .prop_map(|(round, pending_txids)| MigrationPhase::Preparing {
                    round,
                    pending_txids
                }),
            Just(MigrationPhase::Scheduled),
            Just(MigrationPhase::Prepared),
            any::<u64>().prop_map(|residual| MigrationPhase::Complete { residual }),
        ]
    }

    fn arbitrary_migration_state() -> impl Strategy<Value = MigrationState> {
        (
            arbitrary_params(),
            any::<[u8; 32]>(),
            any::<[u8; 32]>(),
            any::<u64>(),
            any::<bool>(),
            any::<bool>(),
            0u32..(1 << 31),
            arbitrary_phase(),
            proptest::collection::vec(arbitrary_part(), 0..12),
        )
            .prop_map(
                |(
                    params,
                    params_hash,
                    plan_hash,
                    committed_at,
                    lazy,
                    immediate,
                    account,
                    phase,
                    mut transfers,
                )| {
                    let bucket_count = u64::from(u32::MAX / params.bucket_modulus);
                    for transfer in &mut transfers {
                        transfer.bucket_index =
                            transfer.bucket_index.map(|bucket| bucket % bucket_count);
                        transfer.anchor_bucket =
                            transfer.anchor_bucket.map(|bucket| bucket % bucket_count);
                    }
                    MigrationState {
                        params,
                        commitment: PlanCommitment {
                            params_hash,
                            plan_hash,
                            committed_at,
                        },
                        strategy: if lazy {
                            SigningStrategy::LazyAtBoundary
                        } else {
                            SigningStrategy::PreSigned
                        },
                        mode: if immediate {
                            MigrationMode::Immediate
                        } else {
                            MigrationMode::Scheduled
                        },
                        account: zip32::AccountId::try_from(account).expect("in range"),
                        phase,
                        transfers,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn round_trips(state in arbitrary_migration_state()) {
            let mut bytes = Vec::new();
            write(&mut bytes, &state).expect("writes");
            let recovered = read(bytes.as_slice()).expect("reads");
            prop_assert_eq!(state, recovered);
        }
    }

    /// A freshly planned mainnet state, the smallest well-formed fixture.
    fn planned_state() -> MigrationState {
        MigrationState {
            params: MigrationParams::provisional(crate::config::ChainType::Mainnet),
            commitment: PlanCommitment {
                params_hash: [0; 32],
                plan_hash: [0; 32],
                committed_at: 0,
            },
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: MigrationPhase::Committed,
            transfers: Vec::new(),
        }
    }

    fn bound_transfer(id: u32, denomination: u64, txid_byte: u8) -> TransferRecord {
        TransferRecord::new(
            TransferId(id),
            denomination,
            BoundNote {
                output_id: OutputId::new(TxId::from_bytes([txid_byte; 32]), 0),
                nullifier: [txid_byte; 32],
                commitment: [txid_byte; 32],
            },
        )
    }

    fn phase_offset(state: &MigrationState) -> usize {
        1 + 4
            + 1
            + 8 * state.params.denominations.len()
            + 3 * 8
            + 3 * 4
            + 8
            + 8
            + 32
            + 32
            + 8
            + 1
            + 4
    }

    const BOUND_TRANSFER_STATE_OFFSET: usize = 4 + 8 + (1 + 32 + 4 + 32 + 32) + 1 + 1 + 1;

    #[test]
    fn the_migration_section_is_written_at_version_5() {
        let mut bytes = Vec::new();
        write(&mut bytes, &planned_state()).expect("writes");
        assert_eq!(INNER_VERSION, 5);
        assert_eq!(
            bytes[0], 5,
            "the section carries its own version byte first"
        );
    }

    #[test]
    fn a_v6_blob_is_refused() {
        let mut bytes = Vec::new();
        write(&mut bytes, &planned_state()).expect("writes");
        bytes[0] = 6;
        let error = read(bytes.as_slice()).expect_err("a section from the future must not read");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn a_v5_round_trip_keeps_previous_txids_and_missed_windows() {
        let mut rescheduled = bound_transfer(0, 5_000_000, 1);
        rescheduled.assign(3).expect("fresh transfers are bound");
        rescheduled
            .mark_signed(TxId::from_bytes([9; 32]), BlockHeight::from_u32(500), None)
            .expect("assigned transfers sign");
        rescheduled.record_attempt();
        rescheduled
            .mark_broadcast()
            .expect("signed transfers broadcast");
        let discarded = rescheduled
            .discard_signature()
            .expect("broadcast transfers discard their signature")
            .expect("a broadcast transfer has a txid");
        rescheduled.record_missed_window();
        rescheduled.reassign(5).expect("expired transfers reassign");
        rescheduled.record_missed_window();
        assert_eq!(rescheduled.previous_txids, vec![discarded]);
        assert_eq!(rescheduled.missed_windows, 2);

        let mut untouched = bound_transfer(1, 2_000_000, 2);
        untouched.assign(4).expect("fresh transfers are bound");

        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![rescheduled.clone(), untouched.clone()];

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        let read_back = read(bytes.as_slice()).expect("a v5 blob reads");
        assert_eq!(read_back, state);
        assert_eq!(read_back.transfers[0].previous_txids, vec![discarded]);
        assert_eq!(read_back.transfers[0].missed_windows, 2);
        assert!(read_back.transfers[1].previous_txids.is_empty());
        assert_eq!(read_back.transfers[1].missed_windows, 0);
    }

    #[test]
    fn the_prepared_phase_is_code_4_and_round_trips() {
        let mut state = planned_state();
        state.phase = MigrationPhase::Prepared;

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        assert_eq!(
            bytes[phase_offset(&state)],
            4,
            "Prepared is written as phase code 4"
        );
        assert_eq!(
            bytes.len() - phase_offset(&state),
            3,
            "Prepared carries no payload: the phase byte, the empty transfer vector, the mode byte"
        );

        let read_back = read(bytes.as_slice()).expect("a Prepared section reads");
        assert_eq!(read_back.phase, MigrationPhase::Prepared);
        assert_eq!(read_back, state);
    }

    #[test]
    fn a_released_transfer_is_code_7_and_round_trips() {
        let mut released = bound_transfer(0, 1_000_000, 3);
        released.mark_released().expect("bound transfers release");
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![released.clone()];

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        let transfer_at = phase_offset(&state) + 1 + 1;
        assert_eq!(
            bytes[transfer_at + BOUND_TRANSFER_STATE_OFFSET],
            7,
            "Released is written as transfer state code 7"
        );

        let read_back = read(bytes.as_slice()).expect("a section with a released transfer reads");
        assert_eq!(read_back.transfers, vec![released]);
        assert_eq!(read_back.transfers[0].state, TransferState::Released);
        assert!(
            read_back.transfers[0].state.is_terminal(),
            "a released transfer is terminal"
        );
    }

    #[test]
    fn every_transfer_state_code_round_trips() {
        let states = [
            TransferState::Bound,
            TransferState::Assigned,
            TransferState::Signed,
            TransferState::Broadcast,
            TransferState::Confirmed {
                height: BlockHeight::from_u32(600),
            },
            TransferState::Expired,
            TransferState::Invalidated,
            TransferState::Released,
        ];
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = states
            .iter()
            .enumerate()
            .map(|(index, transfer_state)| {
                let mut transfer = bound_transfer(index as u32, 1_000_000, index as u8 + 1);
                transfer.state = *transfer_state;
                transfer
            })
            .collect();

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        let read_back = read(bytes.as_slice()).expect("every state code reads");
        let codes: Vec<TransferState> = read_back
            .transfers
            .iter()
            .map(|transfer| transfer.state)
            .collect();
        assert_eq!(codes, states);
    }

    /// Relative to v3, a v1 blob carries the retired `expiry_delta` u32
    /// before `transfer_fee` and lacks the trailing mode byte. Reading it must
    /// succeed, discard the delta, and default the mode to `Scheduled`, the
    /// conservative reading that makes the immediate path refuse to
    /// collapse an old persisted state.
    #[test]
    fn v1_blob_reads_with_scheduled_mode() {
        let mut state = planned_state();
        state.mode = MigrationMode::Immediate;
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 1;
        bytes.pop();
        // Splice the legacy expiry_delta back in where v1 carried it: after
        // the fixed-width params fields that follow the denominations vector.
        let offset = 1 + 4 + 1 + 8 * state.params.denominations.len() + 3 * 8 + 3 * 4 + 8;
        bytes.splice(offset..offset, 296u32.to_le_bytes());

        let recovered = read(bytes.as_slice()).expect("a v1 blob still reads");
        assert_eq!(recovered.mode, MigrationMode::Scheduled);
        state.mode = MigrationMode::Scheduled;
        assert_eq!(recovered, state);
    }

    fn strip_version_5_transfer_tail(bytes: &mut Vec<u8>) {
        let mode_at = bytes.len() - 1;
        assert_eq!(&bytes[mode_at - 5..mode_at], &[0, 0, 0, 0, 0]);
        bytes.drain(mode_at - 5..mode_at);
    }

    #[test]
    fn a_v4_blob_reads_with_no_history() {
        let mut transfer = TransferRecord::new(
            TransferId(0),
            1_000_000,
            BoundNote {
                output_id: OutputId::new(TxId::from_bytes([1; 32]), 0),
                nullifier: [0; 32],
                commitment: [0; 32],
            },
        );
        transfer.assign(3).expect("fresh transfers are bound");
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![transfer.clone()];
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 4;
        strip_version_5_transfer_tail(&mut bytes);

        let read_back = read(bytes.as_slice()).expect("a v4 blob still reads");
        assert_eq!(read_back.phase, MigrationPhase::Scheduled);
        assert_eq!(read_back.transfers, vec![transfer]);
        assert!(read_back.transfers[0].previous_txids.is_empty());
        assert_eq!(read_back.transfers[0].missed_windows, 0);
    }

    #[test]
    fn a_v4_blob_with_a_broadcast_transfer_reads_with_no_history() {
        let mut transfer = bound_transfer(0, 2_000_000, 4);
        transfer.assign(3).expect("fresh transfers are bound");
        transfer.anchor_bucket = Some(2);
        transfer
            .mark_signed(
                TxId::from_bytes([8; 32]),
                BlockHeight::from_u32(69_120),
                None,
            )
            .expect("assigned transfers sign");
        transfer.record_attempt();
        transfer
            .mark_broadcast()
            .expect("signed transfers broadcast");
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![transfer.clone()];
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 4;
        strip_version_5_transfer_tail(&mut bytes);

        let read_back = read(bytes.as_slice()).expect("a v4 blob still reads");
        assert_eq!(read_back.transfers, vec![transfer]);
        assert_eq!(read_back.transfers[0].state, TransferState::Broadcast);
        assert_eq!(read_back.transfers[0].txid, Some(TxId::from_bytes([8; 32])));
        assert_eq!(read_back.transfers[0].attempts, 1);
        assert!(read_back.transfers[0].previous_txids.is_empty());
        assert_eq!(read_back.transfers[0].missed_windows, 0);
    }

    #[test]
    fn a_v4_expired_transfer_keeps_its_txid_and_archives_it_on_reschedule() {
        let stale = TxId::from_bytes([8; 32]);
        let mut transfer = bound_transfer(0, 2_000_000, 4);
        transfer.assign(3).expect("fresh transfers are bound");
        transfer.anchor_bucket = Some(2);
        transfer
            .mark_signed(stale, BlockHeight::from_u32(69_120), None)
            .expect("assigned transfers sign");
        transfer.record_attempt();
        transfer
            .mark_broadcast()
            .expect("signed transfers broadcast");
        transfer.mark_expired().expect("broadcast transfers expire");
        assert_eq!(
            transfer.txid,
            Some(stale),
            "a v4 writer kept the txid on expiry"
        );
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![transfer.clone()];
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 4;
        strip_version_5_transfer_tail(&mut bytes);

        let read_back = read(bytes.as_slice()).expect("a v4 blob still reads");
        let upgraded = &read_back.transfers[0];
        assert_eq!(upgraded.state, TransferState::Expired);
        assert_eq!(upgraded.txid, Some(stale));
        assert!(upgraded.previous_txids.is_empty());
        assert_eq!(upgraded.missed_windows, 0);
        assert!(
            upgraded.owns_txid(&stale),
            "the stale txid is still the transfer's own"
        );

        let mut rescheduled = upgraded.clone();
        rescheduled.reassign(5).expect("expired transfers reassign");
        assert_eq!(rescheduled.state, TransferState::Assigned);
        assert_eq!(rescheduled.txid, None);
        assert_eq!(
            rescheduled.previous_txids,
            vec![stale],
            "the reschedule archives the v4 txid so a late confirmation is recognised"
        );
        assert!(rescheduled.owns_txid(&stale));

        let mut state = read_back.clone();
        state.transfers = vec![rescheduled.clone()];
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        let persisted = read(bytes.as_slice()).expect("a v5 blob reads");
        assert_eq!(persisted.transfers, vec![rescheduled]);
        assert_eq!(persisted.transfers[0].previous_txids, vec![stale]);
    }

    /// Before version 4 a transfer's anchor was its broadcast window's boundary,
    /// an age of zero. Reading such a transfer must sort it by whether a
    /// transaction already commits to that anchor: a `Signed` transfer keeps it,
    /// because its signature cannot be re-aimed, while an unsigned one is left
    /// anchorless so the next placement draws it a legal age. The unsigned
    /// transfer's cached witness must go too — it proves the note under the
    /// window's boundary, so surviving into a redrawn anchor would either fail
    /// to prove or quietly resurrect the age-zero anchor (ADR 0018).
    #[test]
    fn a_v3_blob_keeps_signed_anchors_and_drops_unsigned_ones() {
        let witness = BoundaryWitness {
            anchor: [7; 32],
            position: 11,
            auth_path: vec![[3; 32]],
        };
        // A v3 transfer's anchor *is* its bucket_index, so the fixture writes
        // them equal; the sentinel only has to be locatable in the stream.
        const BUCKET: u64 = 0x00BA_DC0D;
        let with_state = |state: TransferState| {
            let mut transfer = TransferRecord::new(
                TransferId(0),
                1_000_000,
                BoundNote {
                    output_id: OutputId::new(TxId::from_bytes([1; 32]), 0),
                    nullifier: [0; 32],
                    commitment: [0; 32],
                },
            );
            transfer.bucket_index = Some(BUCKET);
            transfer.anchor_bucket = Some(BUCKET);
            transfer.anchor_witness = Some(witness.clone());
            transfer.state = state;
            let mut state_with_part = planned_state();
            state_with_part.transfers = vec![transfer];
            let mut bytes = Vec::new();
            write(&mut bytes, &state_with_part).expect("writes");

            // Re-label as v3 and strip the field the v4 layout added: the
            // `Optional` tag and payload of the *second* of the two adjacent
            // identical encodings, which is `anchor_bucket`.
            bytes[0] = 3;
            strip_version_5_transfer_tail(&mut bytes);
            let field = [&[1u8][..], &BUCKET.to_le_bytes()[..]].concat();
            let first = bytes
                .windows(field.len())
                .position(|window| window == field)
                .expect("bucket_index is in the stream");
            let anchor_at = first + field.len();
            assert_eq!(
                &bytes[anchor_at..anchor_at + field.len()],
                field.as_slice(),
                "anchor_bucket is written immediately after bucket_index"
            );
            bytes.drain(anchor_at..anchor_at + field.len());
            read(bytes.as_slice())
                .expect("a v3 blob still reads")
                .transfers
        };

        let signed = with_state(TransferState::Signed);
        assert_eq!(
            signed[0].anchor_bucket,
            Some(BUCKET),
            "a signed transfer keeps the age-zero anchor its transaction commits"
        );
        assert_eq!(
            signed[0].anchor_witness.as_ref(),
            Some(&witness),
            "and keeps the witness that proves it"
        );

        let assigned = with_state(TransferState::Assigned);
        assert_eq!(
            assigned[0].anchor_bucket, None,
            "an unsigned transfer is left anchorless for a fresh draw"
        );
        assert_eq!(
            assigned[0].anchor_witness, None,
            "and loses the witness aimed at the window's boundary"
        );
    }

    /// The `as usize` cast reading `max_actions_per_split_tx` silently
    /// truncates on 32-bit targets. This test patches a serialized stream so
    /// the persisted value exceeds `u32::MAX`: the read must either reject
    /// the stream (the desired `usize::try_from` + `InvalidData` behavior)
    /// or preserve the value. It passes vacuously on 64-bit hosts. Run under
    /// a 32-bit target (e.g. i686-unknown-linux-gnu) to observe the failure.
    /// Every bucket computation divides or multiplies by the modulus, so a
    /// zero arriving from a corrupt wallet file must fail the read typed
    /// rather than panic downstream bucket arithmetic.
    #[test]
    fn zero_bucket_modulus_is_rejected_at_read() {
        let mut state = planned_state();
        state.params.bucket_modulus = 0;

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");

        let error = read(bytes.as_slice()).expect_err("a zero modulus must not read");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn max_actions_read_never_silently_truncates() {
        const SENTINEL: u64 = 0x1122_3344;
        const PATCHED: u64 = 0x1_1122_3344; // does not fit in u32

        let mut state = planned_state();
        state.params.max_actions_per_split_tx = SENTINEL as usize;

        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");

        // Locate the sentinel's unique little-endian encoding and set a high
        // byte, producing the stream a 64-bit writer (or corruption) would
        // carry.
        let needle = SENTINEL.to_le_bytes();
        let position = bytes
            .windows(8)
            .position(|window| window == needle)
            .expect("sentinel is unique in the fixture");
        bytes[position..position + 8].copy_from_slice(&PATCHED.to_le_bytes());

        match read(bytes.as_slice()) {
            Err(error) => assert_eq!(error.kind(), ErrorKind::InvalidData),
            Ok(read_state) => assert_eq!(
                read_state.params.max_actions_per_split_tx as u64, PATCHED,
                "read silently truncated the persisted value"
            ),
        }
    }

    #[test]
    fn store_rejects_bucket_index_that_overflows_height() {
        const BUCKET: u64 = 1 << 40;
        let mut transfer = TransferRecord::new(
            TransferId(0),
            1_000_000,
            BoundNote {
                output_id: OutputId::new(TxId::from_bytes([7; 32]), 0),
                nullifier: [1; 32],
                commitment: [2; 32],
            },
        );
        transfer.assign(BUCKET).expect("fresh transfers are bound");
        let mut state = planned_state();
        state.phase = MigrationPhase::Scheduled;
        state.transfers = vec![transfer];
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");

        let read_back = read(bytes.as_slice());
        assert!(
            read_back.is_err(),
            "a bucket index no block height can hold must be rejected at read, the way a zero \
             bucket modulus is; instead it loads and panics the first status read: {read_back:?}"
        );
    }
}
