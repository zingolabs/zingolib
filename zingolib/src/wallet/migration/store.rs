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

use super::params::{CANONICAL_EXPIRY_MODULUS, MigrationParams};
use super::parts::{BoundNote, BoundaryWitness, PartId, PartRecord, PartState, SigningStrategy};
use super::{ConsentBinding, MigrationMode, MigrationPhase, MigrationState, QueuedSplitTx};

/// Version of this section's layout, bumped independently of the wallet
/// version. Version 2 appends the [`MigrationMode`] byte after the parts.
///
/// Versions 3 and 4 were spent on divergent development lines and are dead
/// numbers. Version 3 meant two different layouts (a dropped `k_max` slot
/// plus the rolling witness cache on one line; a dropped expiry slot on the
/// other) and neither meaning ever sat at a dev tip, so a v3 blob is refused
/// rather than guessed at. Version 4 shipped from dev: it keeps the
/// `k_max`/`target_sessions` params slots, drops the expiry slot, and adds a
/// per-part `anchor_bucket` beside a single anchor witness.
///
/// Version 5 reunifies the layouts on the ZIP 318 schedule: the params carry
/// the canonical rolling `expiry_modulus` (issue #2519, deviations 3 and 4)
/// and no multiplicity or session slots (deviation 5), and each part carries
/// the boundary-keyed rolling witness cache (deviation 2) in place of any
/// single stored witness. Legacy reads discard the retired slots; a v4
/// read additionally falls back to the provisional expiry modulus and lets
/// the consent params-hash mismatch force a replan.
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
    writer.write_u64::<LittleEndian>(params.max_actions_per_split_tx as u64)?;
    writer.write_u32::<LittleEndian>(params.expiry_modulus)?;
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
            queued,
        } => {
            writer.write_u8(1)?;
            writer.write_u32::<LittleEndian>(*round)?;
            Vector::write(&mut writer, pending_txids, |w, txid| txid.write(w))?;
            // Version 3: the drawn preparation schedule of the round.
            Vector::write(&mut writer, queued, |mut w, tx| {
                Vector::write(&mut w, &tx.inputs, |w, value| {
                    w.write_u64::<LittleEndian>(*value)
                })?;
                Vector::write(&mut w, &tx.outputs, |w, value| {
                    w.write_u64::<LittleEndian>(*value)
                })?;
                w.write_u32::<LittleEndian>(tx.due_height.into())
            })?;
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
pub fn read<R: Read>(mut reader: R) -> io::Result<MigrationState> {
    let inner_version = reader.read_u8()?;
    if inner_version > INNER_VERSION {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unknown migration section version {inner_version}"),
        ));
    }
    // Version 3 named two different development layouts and neither ever
    // sat at a dev tip; a blob claiming it cannot be read unambiguously.
    if inner_version == 3 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "migration section version 3 is ambiguous and never shipped",
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
    // Versions 2 and below, and dev's version 4, carried a `k_max`
    // multiplicity cap and a `target_sessions` signing-session target in
    // two u32 slots here. ZIP 318 places no per-wallet multiplicity cap
    // (issue #2519, deviation 5) and the Poisson schedule (deviation 1)
    // retired the session target, so version 5 drops the slots and a
    // legacy read discards them.
    if inner_version <= 2 || inner_version == 4 {
        let _legacy_k_max = reader.read_u32::<LittleEndian>()?;
        let _legacy_target_sessions = reader.read_u32::<LittleEndian>()?;
    }
    let max_actions_per_split_tx =
        usize::try_from(reader.read_u64::<LittleEndian>()?).map_err(|_| {
            Error::new(
                ErrorKind::InvalidData,
                "max_actions_per_split_tx does not fit in this platform's usize",
            )
        })?;
    // Params version 1 repurposed this u32 slot from the retired
    // boundary-relative `expiry_delta`; the layout is unchanged. Dev's
    // version 4 dropped the slot entirely, so a v4 read falls back to the
    // provisional modulus and relies on the consent params-hash mismatch
    // to force a replan.
    let expiry_modulus = if inner_version == 4 {
        CANONICAL_EXPIRY_MODULUS
    } else {
        reader.read_u32::<LittleEndian>()?
    };
    let part_fee = reader.read_u64::<LittleEndian>()?;
    let params = MigrationParams {
        version,
        denominations,
        denom_cap,
        max_residual_value,
        sweep_min,
        bucket_modulus,
        max_actions_per_split_tx,
        expiry_modulus,
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
            // Only version 5 carries the drawn preparation schedule
            // (version 2 predates it and dev's version 4 never had one);
            // an empty queue makes the driver replan the remaining round.
            let queued = if inner_version < 5 {
                Vec::new()
            } else {
                Vector::read(&mut reader, |mut r| {
                    let inputs = Vector::read(&mut r, |r| r.read_u64::<LittleEndian>())?;
                    let outputs = Vector::read(&mut r, |r| r.read_u64::<LittleEndian>())?;
                    let due_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                    Ok(QueuedSplitTx {
                        inputs,
                        outputs,
                        due_height,
                    })
                })?
            };
            MigrationPhase::NoteSplitting {
                round,
                pending_txids,
                queued,
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

    let parts = Vector::read(&mut reader, |r| read_part(r, inner_version))?;

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
    // Version 3: the rolling witness cache, a boundary-keyed map (the
    // single optional witness of version 2 is discarded on legacy read).
    let witnesses: Vec<(&u32, &BoundaryWitness)> = part.boundary_witnesses.iter().collect();
    Vector::write(&mut writer, &witnesses, |mut w, (boundary, witness)| {
        w.write_u32::<LittleEndian>(**boundary)?;
        w.write_all(&witness.anchor)?;
        w.write_u64::<LittleEndian>(witness.position)?;
        Vector::write(&mut w, &witness.auth_path, |w, node| w.write_all(node))
    })?;
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
    // Dev's version 4 stored a scheduling-time anchor bucket per part; the
    // anchor is now drawn at proving time (ZIP 318's anchor-selection
    // rule), so a v4 read discards it.
    if inner_version == 4 {
        Optional::read(&mut reader, |r| r.read_u64::<LittleEndian>())?;
    }
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
    let boundary_witnesses = if inner_version < 5 {
        // Version 2 stored a single optional witness with no boundary key,
        // and dev's version 4 stored a single scheduling-time anchor
        // witness in the same shape. Both prove against boundaries the
        // proving-time anchor draw will not choose; discard and recapture
        // from the tree after sync.
        Optional::read(&mut reader, |mut r| {
            let mut anchor = [0u8; 32];
            r.read_exact(&mut anchor)?;
            let _position = r.read_u64::<LittleEndian>()?;
            Vector::read(&mut r, |r| {
                let mut node = [0u8; 32];
                r.read_exact(&mut node)?;
                Ok(node)
            })?;
            Ok(())
        })?;
        std::collections::BTreeMap::new()
    } else {
        Vector::read(&mut reader, |mut r| {
            let boundary = r.read_u32::<LittleEndian>()?;
            let mut anchor = [0u8; 32];
            r.read_exact(&mut anchor)?;
            let position = r.read_u64::<LittleEndian>()?;
            let auth_path = Vector::read(&mut r, |r| {
                let mut node = [0u8; 32];
                r.read_exact(&mut node)?;
                Ok(node)
            })?;
            Ok((
                boundary,
                BoundaryWitness {
                    anchor,
                    position,
                    auth_path,
                },
            ))
        })?
        .into_iter()
        .collect()
    };
    let attempts = reader.read_u8()?;

    Ok(PartRecord {
        id,
        denomination,
        note,
        bucket_index,
        target_height,
        state,
        txid,
        expiry_height,
        signed_blob,
        boundary_witnesses,
        attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arbitrary_params() -> impl Strategy<Value = MigrationParams> {
        (
            any::<u32>(),
            proptest::collection::vec(1u64..=10_000_000_000, 1..8),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u32>(),
            1usize..=1000,
            any::<u32>(),
            any::<u64>(),
        )
            .prop_map(
                |(
                    version,
                    denominations,
                    denom_cap,
                    max_residual_value,
                    sweep_min,
                    bucket_modulus,
                    max_actions_per_split_tx,
                    expiry_modulus,
                    part_fee,
                )| MigrationParams {
                    version,
                    denominations,
                    denom_cap,
                    max_residual_value,
                    sweep_min,
                    bucket_modulus,
                    max_actions_per_split_tx,
                    expiry_modulus,
                    part_fee,
                },
            )
    }

    fn arbitrary_txid() -> impl Strategy<Value = TxId> {
        any::<[u8; 32]>().prop_map(TxId::from_bytes)
    }

    fn arbitrary_state() -> impl Strategy<Value = PartState> {
        prop_oneof![
            Just(PartState::Bound),
            Just(PartState::Assigned),
            Just(PartState::Signed),
            Just(PartState::Broadcast),
            any::<u32>().prop_map(|h| PartState::Confirmed {
                height: BlockHeight::from_u32(h)
            }),
            Just(PartState::Expired),
            Just(PartState::Invalidated),
        ]
    }

    fn arbitrary_part() -> impl Strategy<Value = PartRecord> {
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
            ),
        )
            .prop_map(
                |(
                    (id, denomination, note, bucket_index, target_height, state),
                    (txid, expiry_height, signed_blob, anchor_witness, attempts),
                )| PartRecord {
                    id: PartId(id),
                    denomination,
                    note: note.map(|(txid, output_index, nullifier, commitment)| BoundNote {
                        output_id: OutputId::new(txid, output_index),
                        nullifier,
                        commitment,
                    }),
                    bucket_index,
                    target_height: target_height.map(BlockHeight::from_u32),
                    state,
                    txid,
                    expiry_height: expiry_height.map(BlockHeight::from_u32),
                    signed_blob,
                    boundary_witnesses: anchor_witness
                        .map(|(anchor, position, auth_path)| {
                            // Keyed at the provisional boundary modulus — a
                            // named ZIP 318 value, never a bare literal.
                            let modulus =
                                MigrationParams::provisional(crate::config::ChainType::Mainnet)
                                    .bucket_modulus;
                            std::collections::BTreeMap::from([(
                                modulus,
                                BoundaryWitness {
                                    anchor,
                                    position,
                                    auth_path,
                                },
                            )])
                        })
                        .unwrap_or_default(),
                    attempts,
                },
            )
    }

    fn arbitrary_phase() -> impl Strategy<Value = MigrationPhase> {
        prop_oneof![
            Just(MigrationPhase::Planned),
            (
                any::<u32>(),
                proptest::collection::vec(arbitrary_txid(), 0..5)
            )
                .prop_map(|(round, pending_txids)| MigrationPhase::NoteSplitting {
                    round,
                    pending_txids,
                    queued: vec![QueuedSplitTx {
                        inputs: vec![1_020_000, 300_000],
                        outputs: vec![1_020_000],
                        due_height: BlockHeight::from_u32(3_500_000),
                    }],
                }),
            Just(MigrationPhase::PartsScheduled),
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
                    consented_at,
                    lazy,
                    immediate,
                    account,
                    phase,
                    parts,
                )| {
                    MigrationState {
                        params,
                        consent: ConsentBinding {
                            params_hash,
                            plan_hash,
                            consented_at,
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
                        parts,
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
            consent: ConsentBinding {
                params_hash: [0; 32],
                plan_hash: [0; 32],
                consented_at: 0,
            },
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: MigrationPhase::Planned,
            parts: Vec::new(),
        }
    }

    #[test]
    fn unknown_inner_version_is_rejected() {
        let mut bytes = Vec::new();
        write(&mut bytes, &planned_state()).expect("writes");
        bytes[0] = INNER_VERSION + 1;
        assert!(read(bytes.as_slice()).is_err());
    }

    /// Version 3 named two divergent development layouts and neither ever
    /// sat at a dev tip, so a blob claiming it must be refused rather than
    /// parsed under a guessed layout.
    #[test]
    fn ambiguous_v3_is_refused() {
        let mut bytes = Vec::new();
        write(&mut bytes, &planned_state()).expect("writes");
        bytes[0] = 3;
        let error = read(bytes.as_slice()).expect_err("a v3 blob must not read");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    /// Dev's version 4 keeps the `k_max`/`target_sessions` u32 slots and
    /// drops the expiry u32; a v4 blob is therefore the current blob with
    /// the version byte lowered, the two slots re-inserted after
    /// `bucket_modulus`, and the expiry u32 removed. Reading it must
    /// succeed, discard the retired slots, and fall back to the
    /// provisional expiry modulus.
    #[test]
    fn v4_blob_reads_with_legacy_discards() {
        let state = planned_state();
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 4;
        // Offset of the retired slots, as in `v1_blob_reads_with_scheduled_mode`.
        let denominations = state.params.denominations.len();
        assert!(denominations < 253, "CompactSize stays a single byte");
        let legacy_offset = 1 + 4 + (1 + denominations * 8) + 8 + 8 + 8 + 4;
        let mut legacy_slots = Vec::new();
        legacy_slots.extend(8u32.to_le_bytes());
        legacy_slots.extend(6u32.to_le_bytes());
        bytes.splice(legacy_offset..legacy_offset, legacy_slots);
        // Remove the expiry u32 that follows the two inserted slots and
        // the `max_actions_per_split_tx` u64.
        let expiry_offset = legacy_offset + 4 + 4 + 8;
        bytes.drain(expiry_offset..expiry_offset + 4);

        let recovered = read(bytes.as_slice()).expect("a v4 blob still reads");
        assert_eq!(recovered.params.expiry_modulus, CANONICAL_EXPIRY_MODULUS);
        assert_eq!(recovered, state);
    }

    /// Version 2 only appended the mode byte, and version 5 dropped the
    /// legacy `k_max` and `target_sessions` params slots; a v1 blob is
    /// therefore the current blob with the version byte lowered, the two
    /// u32 slots re-inserted after `bucket_modulus`, and the trailing mode
    /// byte dropped. Reading it must succeed and default the mode to
    /// `Scheduled`, the conservative reading that makes the immediate path
    /// refuse to collapse an old persisted state.
    #[test]
    fn v1_blob_reads_with_scheduled_mode() {
        let mut state = planned_state();
        state.mode = MigrationMode::Immediate;
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = 1;
        // Offset of the retired `k_max` slot: the inner version byte, the
        // params version, the denominations vector (a single CompactSize
        // length byte below 253 entries plus eight bytes each), denom_cap,
        // max_residual_value, sweep_min, and bucket_modulus.
        let denominations = state.params.denominations.len();
        assert!(denominations < 253, "CompactSize stays a single byte");
        let legacy_offset = 1 + 4 + (1 + denominations * 8) + 8 + 8 + 8 + 4;
        // The retired `k_max` then `target_sessions` u32 slots.
        let mut legacy_slots = Vec::new();
        legacy_slots.extend(8u32.to_le_bytes());
        legacy_slots.extend(6u32.to_le_bytes());
        bytes.splice(legacy_offset..legacy_offset, legacy_slots);
        bytes.pop();

        let recovered = read(bytes.as_slice()).expect("a v1 blob still reads");
        assert_eq!(recovered.mode, MigrationMode::Scheduled);
        state.mode = MigrationMode::Scheduled;
        assert_eq!(recovered, state);
    }

    /// The `as usize` cast reading `max_actions_per_split_tx` silently
    /// truncates on 32-bit targets. This test patches a serialized stream so
    /// the persisted value exceeds `u32::MAX`: the read must either reject
    /// the stream (the desired `usize::try_from` + `InvalidData` behavior)
    /// or preserve the value. It passes vacuously on 64-bit hosts; run under
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
}
