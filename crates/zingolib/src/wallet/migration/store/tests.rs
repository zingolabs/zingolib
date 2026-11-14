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
        any::<u32>(),
        any::<u32>(),
        1usize..=1000,
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
                k_max,
                target_sessions,
                max_actions_per_split_tx,
                part_fee,
            )| MigrationParams {
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
            proptest::option::of(any::<u64>()),
        ),
    )
        .prop_map(
            |(
                (id, denomination, note, bucket_index, target_height, state),
                (txid, expiry_height, signed_blob, anchor_witness, attempts, anchor_bucket),
            )| PartRecord {
                id: PartId(id),
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
                pending_txids
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
        let recovered = read(bytes.as_slice(), ChainType::Mainnet).expect("reads");
        prop_assert_eq!(state, recovered);
    }
}

/// A chain on which NU7 never activates, so a bucket index reads back as
/// written at every height: the fixture tests' subject is the layout, not
/// the renumbering.
fn uniform_chain() -> ChainType {
    ChainType::Regtest(crate::testutils::mock_activation_heights_with(|era| {
        era.set_nu7(None)
    }))
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
    assert!(read(bytes.as_slice(), ChainType::Mainnet).is_err());
}

/// Relative to v3, a v1 blob carries the retired `expiry_delta` u32
/// before `part_fee` and lacks the trailing mode byte. Reading it must
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

    let recovered = read(bytes.as_slice(), ChainType::Mainnet).expect("a v1 blob still reads");
    assert_eq!(recovered.mode, MigrationMode::Scheduled);
    state.mode = MigrationMode::Scheduled;
    assert_eq!(recovered, state);
}

/// Before version 4 a part's anchor was its transmission window's boundary,
/// an age of zero. Reading such a part must sort it by whether a
/// transaction already commits to that anchor: a `Signed` part keeps it,
/// because its signature cannot be re-aimed, while an unsigned one is left
/// anchorless so the next placement draws it a legal age. The unsigned
/// part's cached witness must go too — it proves the note under the
/// window's boundary, so surviving into a redrawn anchor would either fail
/// to prove or quietly resurrect the age-zero anchor (ADR 0018).
#[test]
fn a_v3_blob_keeps_signed_anchors_and_drops_unsigned_ones() {
    let witness = BoundaryWitness {
        anchor: [7; 32],
        position: 11,
        auth_path: vec![[3; 32]],
    };
    // A v3 part's anchor *is* its bucket_index, so the fixture writes
    // them equal; the sentinel only has to be locatable in the stream.
    const BUCKET: u64 = 0x0BAD_C0DE;
    let with_state = |state: PartState| {
        let mut part = PartRecord::new(
            PartId(0),
            1_000_000,
            BoundNote {
                output_id: OutputId::new(TxId::from_bytes([1; 32]), 0),
                nullifier: [0; 32],
                commitment: [0; 32],
            },
        );
        part.bucket_index = Some(BUCKET);
        part.anchor_bucket = Some(BUCKET);
        part.anchor_witness = Some(witness.clone());
        part.state = state;
        let mut state_with_part = planned_state();
        state_with_part.parts = vec![part];
        let mut bytes = Vec::new();
        write(&mut bytes, &state_with_part).expect("writes");

        // Re-label as v3 and strip the field the v4 layout added: the
        // `Optional` tag and payload of the *second* of the two adjacent
        // identical encodings, which is `anchor_bucket`.
        bytes[0] = 3;
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
        read(bytes.as_slice(), uniform_chain())
            .expect("a v3 blob still reads")
            .parts
    };

    let signed = with_state(PartState::Signed);
    assert_eq!(
        signed[0].anchor_bucket,
        Some(BUCKET),
        "a signed part keeps the age-zero anchor its transaction commits"
    );
    assert_eq!(
        signed[0].anchor_witness.as_ref(),
        Some(&witness),
        "and keeps the witness that proves it"
    );

    let assigned = with_state(PartState::Assigned);
    assert_eq!(
        assigned[0].anchor_bucket, None,
        "an unsigned part is left anchorless for a fresh draw"
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

    let error =
        read(bytes.as_slice(), ChainType::Mainnet).expect_err("a zero modulus must not read");
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

    match read(bytes.as_slice(), ChainType::Mainnet) {
        Err(error) => assert_eq!(error.kind(), ErrorKind::InvalidData),
        Ok(read_state) => assert_eq!(
            read_state.params.max_actions_per_split_tx as u64, PATCHED,
            "read silently truncated the persisted value"
        ),
    }
}
/// A section written before version 5 numbers its buckets on a uniform
/// grid, `height / M`. On a chain where NU7 is active those indices are
/// renumbered onto the chain's grid: a boundary that still opens a bucket
/// keeps its height, a window that does not moves up to the next
/// boundary, and such an anchor is dropped, with its witness unless a
/// transaction commits to it. On a chain without NU7, and for a version 5
/// section, the reading is the identity.
#[test]
fn a_v4_blob_is_renumbered_onto_the_chains_grid() {
    use crate::testutils::mock_activation_heights_with;
    let witness = BoundaryWitness {
        anchor: [7; 32],
        position: 11,
        auth_path: vec![[3; 32]],
    };
    let part_with = |id: u32, state: PartState, bucket: u64, anchor: u64| {
        let mut part = PartRecord::new(
            PartId(id),
            1_000_000,
            BoundNote {
                output_id: OutputId::new(TxId::from_bytes([1; 32]), id),
                nullifier: [0; 32],
                commitment: [0; 32],
            },
        );
        part.bucket_index = Some(bucket);
        part.anchor_bucket = Some(anchor);
        part.anchor_witness = Some(witness.clone());
        part.state = state;
        part
    };
    let blob = |parts: Vec<PartRecord>, inner_version: u8| {
        let mut state = planned_state();
        state.parts = parts;
        let mut bytes = Vec::new();
        write(&mut bytes, &state).expect("writes");
        bytes[0] = inner_version;
        bytes
    };
    // M is 144. With NU7 active from the second block the grid's boundaries
    // are 0 and the multiples of 432: uniform buckets 3 and 6 open grid
    // buckets 1 and 2, uniform buckets 4 and 5 open none.
    let scaled = ChainType::Regtest(mock_activation_heights_with(|era| era.set_nu7(Some(1))));
    let uniform = uniform_chain();
    assert_eq!(planned_state().params.bucket_modulus, 144);

    let recovered = read(
        blob(
            vec![
                part_with(0, PartState::Assigned, 6, 3),
                part_with(1, PartState::Assigned, 5, 4),
                part_with(2, PartState::Signed, 5, 4),
            ],
            4,
        )
        .as_slice(),
        scaled,
    )
    .expect("a v4 blob reads")
    .parts;
    assert_eq!(recovered[0].bucket_index, Some(2));
    assert_eq!(recovered[0].anchor_bucket, Some(1));
    assert_eq!(recovered[0].anchor_witness, Some(witness.clone()));
    assert_eq!(
        recovered[1].bucket_index,
        Some(2),
        "the window moves to the next boundary"
    );
    assert_eq!(
        recovered[1].anchor_bucket, None,
        "an unsigned off-grid anchor is redrawn"
    );
    assert_eq!(recovered[1].anchor_witness, None);
    assert_eq!(recovered[2].bucket_index, Some(2));
    assert_eq!(recovered[2].anchor_bucket, None);
    assert_eq!(
        recovered[2].anchor_witness,
        Some(witness.clone()),
        "a signed part keeps the witness its transaction carries"
    );

    let identity = read(
        blob(vec![part_with(0, PartState::Assigned, 5, 4)], 4).as_slice(),
        uniform,
    )
    .expect("a v4 blob reads")
    .parts;
    assert_eq!(identity[0].bucket_index, Some(5));
    assert_eq!(identity[0].anchor_bucket, Some(4));

    let current = read(
        blob(vec![part_with(0, PartState::Assigned, 5, 4)], INNER_VERSION).as_slice(),
        scaled,
    )
    .expect("a current blob reads")
    .parts;
    assert_eq!(
        current[0].bucket_index,
        Some(5),
        "a version 5 index is already the grid's"
    );
    assert_eq!(current[0].anchor_bucket, Some(4));
}
