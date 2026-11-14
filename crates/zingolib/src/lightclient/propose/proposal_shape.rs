use zcash_protocol::{PoolType, ShieldedPool};

use crate::lightclient::LightClient;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};

/// A pool whose spendable list is exhausted mid-selection must still
/// report the value it covered. The regression this pins: selecting the
/// pool's last candidate note broke out of the selection loop before
/// the remaining-needed figure was recomputed, so the caller saw a
/// stale positive remainder and the next pool selected against a
/// target that was already met. The wallet holds the covering note in
/// Orchard and a larger note in Sapling (the two trailing pools of
/// the selector's preference order for an Ironwood payment), so the
/// assertion is independent of that order's head.
#[tokio::test]
async fn exhausted_pool_does_not_over_select() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(50_000)
        .sapling_note(100_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination =
        synthetic_wallet::external_address(PoolType::Shielded(ShieldedPool::Ironwood));
    let proposal = from_inputs::propose(&mut client, vec![(destination.as_str(), 20_000, None)])
        .await
        .unwrap();
    let selected: Vec<u64> = proposal
        .steps()
        .first()
        .shielded_inputs()
        .expect("a shielded send selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    assert_eq!(
        selected,
        [50_000],
        "one note covers the send; an emptied pool must not spill selection into the next"
    );
}

/// A note bound to a pending migration part is withheld from ordinary
/// input selection while a free note can satisfy the request (the
/// ZIP 318 soft reservation). This is the unit-level twin of the
/// libtonode `bound_note_reservation_and_external_spend_invalidation`
/// scenario, and it fails alongside the exhausted-pool regression
/// above: a stale remainder made the never-block fallback consume the
/// bound note although the free note had already covered the target.
#[tokio::test]
async fn reservation_withholds_bound_note_while_a_free_note_suffices() {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};

    use crate::wallet::migration::{
        BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId,
        PartRecord, SigningStrategy,
    };

    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(100_000)
        .orchard_note(50_000)
        .build();

    let (output_id, nullifier) = wallet
        .wallet_transactions
        .values()
        .flat_map(OrchardNote::transaction_outputs)
        .find(|note| note.value() == 100_000)
        .map(|note| {
            (
                note.output_id(),
                note.nullifier()
                    .expect("scanned notes carry nullifiers")
                    .to_bytes(),
            )
        })
        .expect("the wallet holds the 100_000 note");

    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(MigrationState {
        consent: ConsentBinding {
            params_hash: params.params_hash(),
            plan_hash: [0; 32],
            consented_at: 0,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: MigrationPhase::PartsScheduled,
        parts: vec![PartRecord::new(
            PartId(0),
            100_000,
            BoundNote {
                output_id,
                nullifier,
                commitment: [0; 32],
            },
        )],
    });
    let mut client = LightClient::new_for_test(wallet).await;

    let destination =
        synthetic_wallet::external_address(PoolType::Shielded(ShieldedPool::Ironwood));
    let proposal = from_inputs::propose(&mut client, vec![(destination.as_str(), 20_000, None)])
        .await
        .unwrap();
    let selected: Vec<u64> = proposal
        .steps()
        .first()
        .shielded_inputs()
        .expect("a shielded send selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    assert_eq!(
        selected,
        [50_000],
        "the free note covers the send; the bound note stays reserved"
    );
}

/// Migrated from libtonode `slow::dust_sends_change_correctly`: a send
/// of less than the fee still proposes, since the fee comes out of the
/// selected note and the remainder returns as change. The original
/// asserted nothing beyond the send call succeeding. The offline
/// proposal now asserts the shape the name always promised.
#[tokio::test]
async fn dust_sends_change_correctly() {
    let note_value = 100_000;
    let sent_value = 1_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination = synthetic_wallet::external_address(PoolType::IRONWOOD);
    let proposal =
        from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
            .await
            .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let fee = u64::from(step.balance().fee_required());
    assert_eq!(
        fee,
        fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
    );
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change, note_value - sent_value - fee);
}

/// Offline twin of libtonode
/// `basic_transactions::send_and_sync_with_multiple_notes_no_panic`,
/// which stays live as the pipeline control: a 50_000 payment from
/// two 40_000 notes must gather both (either alone is consumed by
/// the payment plus the 10_000 one-to-one fee) and return exactly
/// 20_000 change.
#[tokio::test]
async fn payment_no_single_note_covers_gathers_both_and_changes() {
    let note_value = 40_000;
    let sent_value = 50_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(note_value)
        .ironwood_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination = synthetic_wallet::external_address(PoolType::IRONWOOD);
    let proposal =
        from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
            .await
            .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let selected: Vec<u64> = step
        .shielded_inputs()
        .expect("a shielded payment selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    assert_eq!(selected, [note_value, note_value], "both notes gathered");
    let fee = u64::from(step.balance().fee_required());
    assert_eq!(
        fee,
        fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
    );
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change, 2 * note_value - sent_value - fee);
    assert_eq!(change, 20_000);
}

/// Offline twin of libtonode `slow::sapling_dust_fee_collection`,
/// which stays live as the pipeline control: from 100_000 orchard
/// plus a 1_000-zat sapling dust note, a 50_000 send selects only
/// the orchard note (dust cannot pay for its own input), charges the
/// plain one-to-one fee, and leaves 40_000 change, the live test's
/// closing balance, derived here at proposal time. The dust note
/// stays behind untouched.
#[tokio::test]
async fn sapling_dust_is_not_collected_toward_fees() {
    let ironwood_value = 100_000;
    let sapling_dust = 1_000;
    let sent_value = 50_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(ironwood_value)
        .sapling_note(sapling_dust)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination = synthetic_wallet::external_address(PoolType::IRONWOOD);
    let proposal =
        from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
            .await
            .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let selected: Vec<u64> = step
        .shielded_inputs()
        .expect("a shielded payment selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    assert_eq!(selected, [ironwood_value], "the dust note is not selected");
    let fee = u64::from(step.balance().fee_required());
    assert_eq!(
        fee,
        fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
    );
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change, ironwood_value - sent_value - fee);
    assert_eq!(change, 40_000);
}

/// Migrated from libtonode `shield_transparent` (long `#[ignore]`d):
/// shielding transparent funds proposes without error, consuming the
/// coin into shielded change minus the fee. The original asserted
/// nothing beyond the operations succeeding, so the offline proposal
/// covers everything it protected.
#[tokio::test]
async fn shield_transparent() {
    let value = 100_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    assert_eq!(step.transparent_inputs().len(), 1);
    let fee = u64::from(step.balance().fee_required());
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change + fee, value);
}

/// Migrated from libtonode `fast::mine_to_transparent_and_propose_shielding`:
/// a four-coin shield proposes as a single step spending all four
/// coins into one change output, with the zip317 fee for four
/// transparent inputs plus the orchard action pair. The original
/// mined the coins and waited out coinbase maturity. Propose logic
/// detects coinbase through the stored transaction's transparent
/// bundle (`spendable_transparent_coins` demands 100 extra
/// confirmations for it), and fabricated records carry no transparent
/// bundle, so these coins present as ordinary mature ones and the
/// proposal shape is identical.
#[tokio::test]
async fn four_coin_shield_proposal_shape() {
    let coin_value = 1_000_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    assert_eq!(step.transparent_inputs().len(), 4);
    assert_eq!(u64::from(step.balance().fee_required()), 30_000);
    assert_eq!(step.balance().proposed_change().len(), 1);
    assert_eq!(
        u64::from(step.balance().proposed_change()[0].value()),
        4 * coin_value - 30_000
    );
}

/// Migrated from libtonode `slow::zero_value_change`: a send that
/// leaves exactly zero after the fee still proposes an orchard change
/// output of value zero. The original mined the transaction and then
/// counted one unspent zero-value note and one confirmed-spent funding
/// note. The note-state bookkeeping it exercised is covered by the
/// pepper-sync spend-status rig, and the load-bearing claim (the
/// builder emits the zero-value change) is decided at proposal time.
#[tokio::test]
async fn zero_value_change() {
    let note_value = 100_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true);
    let destination = synthetic_wallet::external_address(PoolType::IRONWOOD);
    let proposal = from_inputs::propose(
        &mut client,
        vec![(destination.as_str(), note_value - fee, None)],
    )
    .await
    .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    assert_eq!(u64::from(step.balance().fee_required()), fee);
    let change = step.balance().proposed_change();
    assert_eq!(change.len(), 1);
    assert_eq!(u64::from(change[0].value()), 0);
    assert_eq!(change[0].output_pool(), PoolType::IRONWOOD);
}

/// Migrated from libtonode `slow::zero_value_change_to_orchard_created`:
/// same zero-value-change arithmetic on a cross-pool send. An 80_000
/// payment to an external sapling address out of a 100_000 orchard note
/// costs exactly the 20_000 cross-pool fee, so the ironwood change
/// output is proposed at value zero.
#[tokio::test]
async fn zero_value_change_to_ironwood_created() {
    let note_value = 100_000;
    let sent_value = 80_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination = synthetic_wallet::external_address(PoolType::SAPLING);
    let proposal =
        from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
            .await
            .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::SAPLING, true);
    assert_eq!(u64::from(step.balance().fee_required()), fee);
    assert_eq!(note_value - sent_value - fee, 0);
    let change = step.balance().proposed_change();
    assert_eq!(change.len(), 1);
    assert_eq!(u64::from(change[0].value()), 0);
    assert_eq!(change[0].output_pool(), PoolType::IRONWOOD);
}

/// Spending a legacy Orchard note on a post-NU6.3 chain must leave the
/// change in the **Orchard** pool, not route it to Ironwood. ZIP 318
/// disables ordinary *payments* into the Orchard pool once NU6.3 activates
/// "while still permitting change"; the turnstile (ZIP 2006) blocks value
/// *entering* Orchard, and change funded by an Orchard input never leaves
/// it, so an Orchard change output nets the pool down and stays
/// consensus-valid. Keeping change in the source pool is the privacy policy
/// for ordinary sends (a discussed decision): it reveals only the payment
/// value crossing pools rather than migrating the whole note, and avoids
/// stamping every send with an Orchard->Ironwood migration fingerprint.
/// Deliberate Orchard->Ironwood movement is the migration engine's job, not
/// an ordinary send's.
#[tokio::test]
async fn orchard_send_change_stays_orchard_post_nu6_3() {
    let note_value = 200_000;
    let sent_value = 50_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    // Post-NU6.3 the payment itself routes through Ironwood (ZIP 318), but
    // that must not drag the change out of Orchard.
    let destination = synthetic_wallet::external_address(PoolType::IRONWOOD);
    let proposal =
        from_inputs::propose(&mut client, vec![(destination.as_str(), sent_value, None)])
            .await
            .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let change = step.balance().proposed_change();
    assert_eq!(change.len(), 1);
    assert!(
        u64::from(change[0].value()) > 0,
        "the send must leave real change for the pool assertion to bite"
    );
    assert_eq!(
        change[0].output_pool(),
        PoolType::ORCHARD,
        "change from spending a legacy Orchard note must stay in Orchard \
             post-NU6.3: ZIP 318 permits Orchard change, and the privacy policy \
             keeps it in the source pool instead of migrating on every send"
    );
}

/// Migrated from libtonode `fast::tex::send_to_tex`: a payment to a
/// TEX address proposes as the ZIP-320 two-step: shield to an
/// ephemeral transparent output, then the transparent leg to the TEX
/// destination funded entirely by step one. The original's only
/// proposal-level assertion was the step count. The offline version
/// also pins the inter-step wiring. The transmission half (both steps
/// mined, three wallet records) retires with the LocalNet original.
#[tokio::test]
async fn send_to_tex() {
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_protocol::value::Zatoshis;

    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(5_000_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    // A TEX destination derived from an external wallet's first
    // transparent address, as ZIP 320 prescribes.
    let tex_address = synthetic_wallet::external_tex_address();

    let request = TransactionRequest::new(vec![Payment::without_memo(
        zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
        Zatoshis::from_u64(100_000).unwrap(),
    )])
    .unwrap();
    let proposal = client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 2);
    let transparent_leg = proposal.steps().last();
    assert!(
        !transparent_leg.prior_step_inputs().is_empty(),
        "the transparent leg spends step one's ephemeral output"
    );
}

/// Migrated from the chain_generics `ignore_dust_inputs` fixture's
/// load-bearing half: note selection excludes dust inputs. From a
/// wallet holding four 1_000-zat dust notes and one 50_000-zat note
/// in each shielded pool, a 10_000-zat send selects exactly the one
/// viable ironwood note and none of the dust.
#[tokio::test]
async fn dust_inputs_are_ignored() {
    let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
    let wallet = [1_000, 1_000, 1_000, 1_000, 50_000]
        .into_iter()
        .fold(builder, |builder, value| {
            builder.ironwood_note(value).sapling_note(value)
        })
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination =
        synthetic_wallet::external_address(PoolType::Shielded(ShieldedPool::Ironwood));
    let proposal = from_inputs::propose(&mut client, vec![(destination.as_str(), 10_000, None)])
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let selected_values: Vec<u64> = step
        .shielded_inputs()
        .expect("a shielded-funds send selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    assert_eq!(
        selected_values,
        [50_000],
        "exactly the viable ironwood note, none of the dust"
    );
    assert_eq!(
        u64::from(step.balance().fee_required()),
        fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true)
    );
}

/// Migrated from the chain_generics `note_selection_order` fixture's
/// load-bearing half: from notes of 10/20/30/40 thousand zats, a
/// 40_000-zat send selects the two-note covering set that leaves the
/// least change, not more notes and not a higher-value covering set.
#[tokio::test]
async fn note_selection_covers_target_with_minimal_change() {
    let builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
    let wallet = [10_000, 20_000, 30_000, 40_000]
        .into_iter()
        .fold(builder, |builder, value| builder.sapling_note(value))
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let destination =
        synthetic_wallet::external_address(PoolType::Shielded(ShieldedPool::Ironwood));
    let proposal = from_inputs::propose(&mut client, vec![(destination.as_str(), 40_000, None)])
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let selected_values: Vec<u64> = step
        .shielded_inputs()
        .expect("a shielded-funds send selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    let selected_total: u64 = selected_values.iter().sum();
    let fee = u64::from(step.balance().fee_required());
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(selected_values.len(), 2, "selected: {selected_values:?}");
    assert_eq!(selected_total, 40_000 + fee + change);
    // The fixture's guarantee: with 10_000-granular notes available,
    // any selection leaving 10_000 or more change used a bigger note
    // than necessary.
    assert!(change < 10_000, "change {change} implies oversized inputs");
}
