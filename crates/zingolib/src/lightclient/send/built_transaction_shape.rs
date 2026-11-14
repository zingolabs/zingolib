use zcash_protocol::PoolType;
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zingo_common_components::protocol::ActivationHeights;
use zl_common::status::ConfirmationStatus;

use crate::lightclient::LightClient;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};
use crate::wallet::LightWallet;

/// Builds (without transmitting) one send-all from the given wallet
/// and returns the stored transaction's (target, expiry, branch id).
async fn build_one_send(wallet: LightWallet) -> (u32, u32, BranchId) {
    let mut client = LightClient::new_for_test(wallet).await;
    let proposal = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
            None,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();
    let txids = client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(txids.len(), 1);
    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let record = wallet.wallet_transactions.get(&txids[0]).unwrap();
    let ConfirmationStatus::Calculated(target) = record.status() else {
        panic!("a built, untransmitted transaction is stored as Calculated");
    };
    let transaction = record.transaction();
    (
        u32::from(target),
        u32::from(transaction.expiry_height()),
        transaction.consensus_branch_id(),
    )
}

/// The plain cell: synced to the default tip, the build targets
/// tip + 1, expires the standard forty blocks later, and commits to
/// the branch id in force at the target.
#[tokio::test]
async fn expiry_and_branch_id_derive_from_synced_height() {
    let tip = 20;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(100_000)
        .tip(tip)
        .build();
    let chain = wallet.chain_type();

    let (target, expiry, branch_id) = build_one_send(wallet).await;

    assert_eq!(target, tip + 1);
    assert_eq!(
        expiry,
        target + crate::wallet::expiry::NU7_TX_EXPIRY_DELTA,
        "the in-process era is NU7-active, so the NU7 expiry delta applies"
    );
    assert_eq!(
        branch_id,
        BranchId::for_height(&chain, BlockHeight::from_u32(tip + 1))
    );
}

/// Offline twin of libtonode `fast::mine_to_transparent_and_shield`,
/// which stays live as the pipeline control (its coinbase provenance
/// and the documented shield-eligibility race are inexpressible
/// offline): four transparent coins shield in one step, and the
/// built transaction nets exactly their sum minus the 30_000
/// four-input shield fee into the Ironwood pool (a V6 shield's
/// change lands in the ironwood bundle, ADR 0009). The live assert
/// is the post-confirmation balance. The offline equivalent is the
/// ironwood bundle's value balance on the built transaction.
#[tokio::test]
async fn four_coin_shield_builds_and_nets_input_minus_fee() {
    let coin_value = 1_000_000u64;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .transparent_coin(coin_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
    let txids = client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(txids.len(), 1);

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let transaction = wallet
        .wallet_transactions
        .get(&txids[0])
        .unwrap()
        .transaction();
    let transparent = transaction
        .transparent_bundle()
        .expect("a shield spends transparent coins");
    assert_eq!(transparent.vin.len(), 4, "all four coins consumed");
    assert!(
        transparent.vout.is_empty(),
        "a shield pays no transparent outputs"
    );
    let ironwood = transaction
        .ironwood_bundle()
        .expect("a V6 shield produces ironwood change");
    // Negative value balance is value flowing INTO the ironwood pool:
    // the four coins minus the 30_000 zip317 fee (four transparent
    // inputs plus the ironwood action pair).
    assert_eq!(
        i64::from(ironwood.value_balance()),
        -i64::try_from(4 * coin_value - 30_000).unwrap()
    );
}

/// Gap-1b cell of the remediation plan, mirroring the live
/// multi_input_sapling_send_with_orchard_change_no_panic offline: a
/// payment that no single sapling note covers builds (proves) a
/// two-input sapling spend. Under V6 the change stays in Sapling (the
/// upstream change selector avoids pool-crossing when no orchard
/// flow exists, ADR 0009) while the payment to the orchard receiver
/// lands in the ironwood bundle. The sapling proving parameters are
/// embedded in the crate, so the plan's parameters precondition is
/// satisfied in the unit environment.
#[tokio::test]
async fn two_input_sapling_spend_with_sapling_change_builds_offline() {
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_protocol::value::Zatoshis;

    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .sapling_note(20_000)
        .sapling_note(30_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    // 25_000 plus the 20_000 ZIP-317 fee (two sapling spends covering
    // the change, plus the ironwood payment pair) exceeds either note
    // alone, so both are gathered and 5_000 returns as sapling change.
    let request = TransactionRequest::new(vec![Payment::without_memo(
        synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
        Zatoshis::const_from_u64(25_000),
    )])
    .unwrap();
    let proposal = client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .unwrap();
    let step = proposal.steps().first();
    let change = step.balance().proposed_change();
    assert_eq!(change.len(), 1);
    assert_eq!(u64::from(change[0].value()), 5_000);
    assert_eq!(change[0].output_pool(), zcash_protocol::PoolType::IRONWOOD,);

    let txids = client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(txids.len(), 1);

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let transaction = wallet
        .wallet_transactions
        .get(&txids[0])
        .unwrap()
        .transaction();
    let sapling_bundle = transaction
        .sapling_bundle()
        .expect("spending sapling notes produces a sapling bundle");
    assert_eq!(
        sapling_bundle.shielded_spends().len(),
        2,
        "both fabricated sapling notes are spent"
    );
    // The sapling bundle carries the spends and the change output;
    // the payment to the orchard receiver lands in the ironwood
    // bundle, and no legacy orchard bundle exists.
    assert!(
        transaction.orchard_bundle().is_none(),
        "no orchard flow, no orchard bundle"
    );
    let ironwood_bundle = transaction
        .ironwood_bundle()
        .expect("the payment to the orchard receiver produces an ironwood bundle");
    assert_eq!(
        ironwood_bundle.actions().len(),
        2,
        "payment plus dummy padding, the bundle minimum"
    );
}

/// Privacy invariant: spending a legacy (V5) Orchard note on a post-NU6.3
/// chain must keep the **change** in the Orchard pool as a real Orchard
/// output, not migrate it to Ironwood. ZIP 318 disables ordinary *payments*
/// into the Orchard pool "while still permitting change"; the turnstile
/// (ZIP 2006) only blocks value *entering* Orchard, and change funded by an
/// Orchard input nets the pool *down*, so the Orchard change output is
/// consensus-valid. Upstream's `SingleOutputChangeStrategy` already keeps
/// change in the spent note's pool to avoid a pool-crossing (the
/// `ShieldedPool::Ironwood` passed by the proposer is only the *fallback*
/// for transactions with no shielded inputs, e.g. shields, ADR 0009);
/// deliberate Orchard->Ironwood movement is the migration engine's job.
///
/// The proof is at the built-transaction level, not the proposal label:
/// the Orchard bundle carries the spend *and* the change output (its value
/// balance nets out only the payment plus fee, so the change stays in the
/// pool), while only the payment to the recipient's Orchard receiver routes
/// through the Ironwood bundle (turnstiled per ZIP 318). Had the change
/// crossed to Ironwood, the Orchard value balance would equal the whole
/// spent note.
#[tokio::test]
async fn orchard_note_send_keeps_change_in_orchard_pool_post_nu6_3() {
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_protocol::{PoolType, value::Zatoshis};

    let note_value = 200_000u64;
    let sent_value = 50_000u64;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(note_value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let request = TransactionRequest::new(vec![Payment::without_memo(
        synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
        Zatoshis::const_from_u64(sent_value),
    )])
    .unwrap();
    let proposal = client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .unwrap();
    let step = proposal.steps().first();
    let change = step.balance().proposed_change();
    assert_eq!(change.len(), 1);
    let change_value = u64::from(change[0].value());
    assert!(change_value > 0, "the send must leave real change to place");
    assert_eq!(
        change[0].output_pool(),
        PoolType::ORCHARD,
        "the proposer must select the Orchard pool for change when spending an Orchard note"
    );

    let txids = client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(txids.len(), 1);
    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let transaction = wallet
        .wallet_transactions
        .get(&txids[0])
        .unwrap()
        .transaction();

    // Orchard bundle: spends the note and holds the change output. Its
    // value balance is (note - change) = payment + fee, which is strictly
    // less than the whole note — the discriminator proving the change is a
    // real Orchard output that stayed in the pool rather than crossing to
    // Ironwood (which would leave the value balance at the full note value).
    let orchard = transaction
        .orchard_bundle()
        .expect("spending a legacy Orchard note keeps a real Orchard bundle");
    assert_eq!(
        i64::from(*orchard.value_balance()),
        i64::try_from(note_value - change_value).unwrap(),
        "Orchard value balance must net out only payment + fee, leaving the change in Orchard"
    );

    // Ironwood bundle: carries only the payment to the recipient's Orchard
    // receiver, turnstiled per ZIP 318 — never the change.
    let ironwood = transaction
        .ironwood_bundle()
        .expect("the payment to an Orchard receiver routes via Ironwood post-NU6.3");
    assert_eq!(
        i64::from(*ironwood.value_balance()),
        -i64::try_from(sent_value).unwrap(),
        "Ironwood carries only the payment value, not the change"
    );
}

/// Gap-3 cell of the remediation plan: the entire ZIP-320 two-step
/// builds offline behind the seam: `zcash_client_backend` chains
/// step one's ephemeral transparent output into step two before
/// anything touches a network. Step two's sole transparent input
/// spends step one's ephemeral output, and the TEX-decoded P2PKH
/// address receives the payment. The only class left live is zebra's
/// mempool accepting the chained unmined pair.
#[tokio::test]
async fn tex_two_step_chains_ephemeral_output_offline() {
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_protocol::value::Zatoshis;
    use zcash_transparent::address::TransparentAddress;

    let payment_value = 100_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(5_000_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    // A TEX destination derived from an external wallet's first
    // transparent address, as ZIP 320 prescribes.
    let taddr_bytes = synthetic_wallet::external_transparent_pubkey_hash();
    let tex_address = synthetic_wallet::external_tex_address();

    let request = TransactionRequest::new(vec![Payment::without_memo(
        zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
        Zatoshis::from_u64(payment_value).unwrap(),
    )])
    .unwrap();
    let proposal = client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .unwrap();
    let txids = client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(txids.len(), 2, "the ZIP-320 pair builds as two steps");

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let step_one = wallet.wallet_transactions.get(&txids[0]).unwrap();
    let step_two = wallet.wallet_transactions.get(&txids[1]).unwrap();

    let step_two_transparent = step_two
        .transaction()
        .transparent_bundle()
        .expect("the transparent leg carries a transparent bundle");
    assert_eq!(step_two_transparent.vin.len(), 1);
    let prevout = step_two_transparent.vin[0].prevout();
    assert_eq!(
        *prevout.txid(),
        txids[0],
        "step two's sole input spends step one"
    );
    let step_one_bundle = step_one
        .transaction()
        .transparent_bundle()
        .expect("the shield leg pays out an ephemeral transparent output");
    let ephemeral_output = step_one_bundle
        .vout
        .get(prevout.n() as usize)
        .expect("the spent index exists among step one's outputs");
    assert_eq!(
        step_two_transparent.vout.len(),
        1,
        "the transparent leg pays the TEX destination and nothing else"
    );
    // One transparent input, one transparent output: the ZIP-317 fee
    // is the two-action grace minimum, 10_000 zats.
    assert_eq!(
        u64::from(ephemeral_output.value()),
        payment_value + 10_000,
        "step one's ephemeral output funds step two's payment plus its fee exactly"
    );

    let expected_script: zcash_transparent::address::Script =
        TransparentAddress::PublicKeyHash(taddr_bytes)
            .script()
            .into();
    let tex_payment = step_two_transparent
        .vout
        .iter()
        .find(|out| *out.script_pubkey() == expected_script)
        .expect("one of step two's outputs pays the TEX-decoded p2pkh");
    assert_eq!(u64::from(tex_payment.value()), payment_value);
}

/// The boundary cell the tip_spend_rejection attribution isolated:
/// a wallet synced to activation − 1 builds a transaction targeting
/// the activation height, so it must commit to the POST-activation
/// branch id, and to the expiry delta of that branch, `expected_delta`.
/// `heights` must activate a branch at `boundary`, and the wallet must
/// hold a note spendable on both sides of it.
async fn assert_boundary_adjacent_build_uses_post_activation_branch_id(
    heights: ActivationHeights,
    boundary: u32,
    expected_delta: u32,
    wallet: SyntheticWalletBuilder,
) {
    let wallet = wallet.tip(boundary - 1).activation_heights(heights).build();
    let chain = wallet.chain_type();
    let pre_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary - 1));
    let post_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary));
    assert_ne!(
        pre_activation, post_activation,
        "the cell must sit on a real branch boundary"
    );

    let (target, expiry, branch_id) = build_one_send(wallet).await;

    assert_eq!(target, boundary);
    assert_eq!(branch_id, post_activation);
    assert_eq!(expiry, target + expected_delta);
}

/// The permanent unit fence for the wallet-side wrong-branch-id failure
/// observed live at the height-5 NU6.1/6.2 co-activation.
#[tokio::test]
async fn boundary_adjacent_build_uses_post_activation_branch_id() {
    let boundary = 10;
    let heights = crate::testutils::mock_activation_heights_with(|era| {
        era.set_nu6_2(Some(boundary)).set_nu6_3(None).set_nu7(None)
    });
    assert_boundary_adjacent_build_uses_post_activation_branch_id(
        heights,
        boundary,
        zcash_primitives::transaction::builder::BLOSSOM_DEFAULT_TX_EXPIRY_DELTA,
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000),
    )
    .await;
}

/// The same fence at the NU7 boundary, which mainnet crosses on
/// 2026-11-05: an Ironwood note spent from NU7 − 1 commits to the NU7
/// branch id, the id the previous dependency cohort did not know.
#[tokio::test]
async fn nu7_boundary_adjacent_build_uses_post_activation_branch_id() {
    let boundary = 10;
    let heights = crate::testutils::mock_activation_heights_with(|era| era.set_nu7(Some(boundary)));
    assert_boundary_adjacent_build_uses_post_activation_branch_id(
        heights,
        boundary,
        crate::wallet::expiry::NU7_TX_EXPIRY_DELTA,
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(100_000),
    )
    .await;
}
