use super::*;

fn bound_note() -> BoundNote {
    BoundNote {
        output_id: OutputId::new(TxId::from_bytes([7; 32]), 0),
        nullifier: [1; 32],
        commitment: [2; 32],
    }
}

/// Issue #2493, finding 7 (ratified form): placement goes through the
/// schedule module's operations, and the jittered one draws a fresh
/// target inside the new window, see
/// `schedule::tests::place_draws_a_fresh_target_inside_the_new_window`.
/// The raw `reassign` transition deliberately does not manage the
/// target. Placement does. Here, the full complement: every state
/// except `Expired` refuses the transition.
#[test]
fn reassign_is_reachable_only_from_expired() {
    for state in ALL_STATES {
        let is_expired = matches!(state, PartState::Expired);
        let mut part = part_in(state);
        if is_expired {
            part.reassign(5).expect("expired parts reassign");
        } else {
            assert!(
                part.reassign(5).is_err(),
                "reassign must be illegal from {:?}",
                part.state,
            );
        }
    }
}

fn part_in(state: PartState) -> PartRecord {
    let mut part = PartRecord::new(PartId(0), 100_000, bound_note());
    part.state = state;
    part
}

const ALL_STATES: [PartState; 7] = [
    PartState::Bound,
    PartState::Assigned,
    PartState::Signed,
    PartState::Broadcast,
    PartState::Confirmed {
        height: BlockHeight::from_u32(100),
    },
    PartState::Expired,
    PartState::Invalidated,
];

/// Exhaustive legal/illegal edge matrix over every (state, transition)
/// pair.
#[test]
fn transition_matrix() {
    for from in ALL_STATES {
        let legal_assign = matches!(from, PartState::Bound);
        assert_eq!(
            part_in(from).assign(3).is_ok(),
            legal_assign,
            "assign from {from:?}"
        );

        let legal_reassign = matches!(from, PartState::Expired);
        assert_eq!(
            part_in(from).reassign(4).is_ok(),
            legal_reassign,
            "reassign from {from:?}"
        );

        let legal_signed = matches!(from, PartState::Assigned);
        assert_eq!(
            part_in(from)
                .mark_signed(TxId::from_bytes([9; 32]), BlockHeight::from_u32(500), None)
                .is_ok(),
            legal_signed,
            "mark_signed from {from:?}"
        );

        let legal_transmission = matches!(from, PartState::Signed);
        assert_eq!(
            part_in(from).mark_broadcast().is_ok(),
            legal_transmission,
            "mark_broadcast from {from:?}"
        );

        let legal_confirmed = matches!(
            from,
            PartState::Assigned | PartState::Signed | PartState::Broadcast | PartState::Expired
        );
        assert_eq!(
            part_in(from)
                .mark_confirmed(BlockHeight::from_u32(600))
                .is_ok(),
            legal_confirmed,
            "mark_confirmed from {from:?}"
        );

        let legal_expired = matches!(
            from,
            PartState::Assigned | PartState::Signed | PartState::Broadcast
        );
        assert_eq!(
            part_in(from).mark_expired().is_ok(),
            legal_expired,
            "mark_expired from {from:?}"
        );

        let legal_invalidated =
            !matches!(from, PartState::Confirmed { .. } | PartState::Invalidated);
        assert_eq!(
            part_in(from).mark_invalidated().is_ok(),
            legal_invalidated,
            "mark_invalidated from {from:?}"
        );
    }
}

#[test]
fn reassign_clears_stale_transaction_artifacts() {
    let mut part = part_in(PartState::Assigned);
    part.mark_signed(
        TxId::from_bytes([9; 32]),
        BlockHeight::from_u32(500),
        Some(vec![1, 2, 3]),
    )
    .unwrap();
    part.record_attempt();
    part.mark_broadcast().unwrap();
    part.mark_expired().unwrap();

    part.reassign(7).unwrap();
    assert_eq!(part.state, PartState::Assigned);
    assert_eq!(part.bucket_index, Some(7));
    assert_eq!(part.txid, None);
    assert_eq!(part.expiry_height, None);
    assert_eq!(part.signed_blob, None);
    // The note binding and attempt history survive the rebuild.
    assert!(part.note.is_some());
    assert_eq!(part.attempts, 1);
}

#[test]
fn failed_transition_leaves_the_record_untouched() {
    let mut part = part_in(PartState::Bound);
    let before = part.clone();
    assert!(part.mark_broadcast().is_err());
    assert_eq!(part, before);
}

/// A regtest wallet on a chain that never activates NU6.3: the custom-chain
/// shape whose ambient post-sync refresh must not block synchronization
/// (branch `fix/refresh_part_witnesses`).
fn no_nu6_3_wallet() -> crate::wallet::LightWallet {
    let heights = zingo_common_components::protocol::ActivationHeights::builder()
        .set_overwinter(Some(1))
        .set_sapling(Some(1))
        .set_blossom(Some(1))
        .set_heartwood(Some(1))
        .set_canopy(Some(1))
        .set_nu5(Some(1))
        .set_nu6(Some(1))
        .set_nu6_1(Some(1))
        .set_nu6_2(Some(1))
        .set_nu6_3(None)
        .set_nu7(None)
        .build();
    crate::testutils::synthetic_wallet::SyntheticWalletBuilder::new(
        zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED,
    )
    .activation_heights(heights)
    .build()
}

fn scheduled_state(parts: Vec<PartRecord>) -> crate::wallet::migration::MigrationState {
    let params =
        crate::wallet::migration::MigrationParams::provisional(crate::config::ChainType::Mainnet);
    crate::wallet::migration::MigrationState {
        consent: crate::wallet::migration::ConsentBinding {
            params_hash: params.params_hash(),
            plan_hash: [0; 32],
            consented_at: 0,
        },
        params,
        strategy: crate::wallet::migration::SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: crate::wallet::migration::MigrationPhase::PartsScheduled,
        parts,
    }
}

/// The inert arm: with no migration state the refresh returns without
/// consulting the activation schedule, so every sync on a chain without
/// NU6.3 succeeds. This is the bug the branch fixes — before it, the
/// lookup ran unconditionally and failed every `await_sync`.
#[test]
fn refresh_without_migration_state_is_inert_on_no_nu6_3_chain() {
    let mut wallet = no_nu6_3_wallet();
    wallet
        .refresh_part_witnesses()
        .expect("a wallet with no migration state must sync on a no-NU6.3 chain");
}

/// The gate's grain: migration state whose parts carry no witness work
/// (nothing Assigned-and-unwitnessed) is likewise inert, so terminal or
/// fully-witnessed migration history never blocks synchronization even
/// if the activation lookup breaks.
#[test]
fn refresh_without_actionable_parts_is_inert_on_no_nu6_3_chain() {
    let mut wallet = no_nu6_3_wallet();
    wallet.migration = Some(scheduled_state(vec![part_in(PartState::Bound)]));
    wallet
        .refresh_part_witnesses()
        .expect("migration state without witness work must not consult the activation");
}

/// The loud arm: an Assigned part awaiting its witness on a chain with
/// no NU6.3 activation can only mean a real fault (the start paths
/// refuse on such chains), so the refresh errors instead of silently
/// stalling the migration.
#[test]
fn refresh_with_pending_work_errors_on_no_nu6_3_chain() {
    let mut wallet = no_nu6_3_wallet();
    let mut part = PartRecord::new(PartId(0), 100_000, bound_note());
    part.assign(3).expect("bound parts assign");
    wallet.migration = Some(scheduled_state(vec![part]));
    assert!(
        matches!(
            wallet.refresh_part_witnesses(),
            Err(WalletError::MigrationBuild(_))
        ),
        "pending witness work with no NU6.3 activation must error loudly"
    );
}
