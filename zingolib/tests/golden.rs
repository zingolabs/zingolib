#![forbid(unsafe_code)]
#![cfg(all(feature = "perspective", feature = "testutils"))]
//! Golden-JSON regression harness for the editorial surface.
//!
//! These goldens were captured from zingolib's PRE-extraction editorial
//! implementation and pin the consumer contract bit-for-bit: the JSON and
//! Display renderings of `value_transfers`, `messages_containing`, and the
//! three `do_total_*` rollups. The extraction increments must reproduce
//! them exactly; a diff here is a compatibility break, never a fixture to
//! update. The capture mechanism was removed after the original capture,
//! so these files are never regenerated: a contract change that is truly
//! intended must re-mint its golden rows by hand, in review.
//!
//! One deliberate softening: the `do_total_*` structs are HashMap-backed,
//! so their JSON key order is random per process — for those the harness
//! canonicalizes object key order before comparison. Value equality is the
//! real contract there; consumers see random key order today too.

#[path = "support/common.rs"]
mod common;

use std::str::FromStr as _;

use pepper_sync::wallet::{IronwoodNote, OutgoingIronwoodNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::Memo;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingo_test_vectors::seeds;
use zingolib::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use zingolib::config::ChainType;
use zingolib::mocks::orchard_note::OrchardCryptoNoteBuilder;
use zingolib::wallet::LightWallet;
use zingolib::wallet::keys::unified::ReceiverSelection;

use common::{received, regtest_wallet, sent};

/// A wallet exercising every editorial classification the goldens pin:
/// received text/empty/arbitrary memos, a plain send with a memo, a
/// memo-to-self on a sending transaction, a basic send-to-self, and the
/// Zennies-for-Zingo dual-output self-send.
fn editorial_wallet() -> LightWallet {
    let mut alice_wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
    let network = ChainType::Regtest(ActivationHeights::default());

    let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, bob) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();

    let own_orchard_receiver = *alice_wallet
        .unified_addresses()
        .values()
        .next()
        .unwrap()
        .orchard()
        .unwrap();
    let zcash_client_backend::address::Address::Unified(zfz_unified_address) =
        zcash_client_backend::address::Address::decode(&network, ZENNIES_FOR_ZINGO_REGTEST_ADDRESS)
            .unwrap()
    else {
        panic!("ZFZ address must be unified");
    };

    // tx1: received, one text memo alongside an empty one.
    let tx1 = received(
        1,
        10,
        &[Memo::from_str("Hello Alice").unwrap(), Memo::Empty],
    );
    // tx2: received, a non-text (arbitrary bytes) memo — today's editorial
    // layer silently renders this as no memo at all; the golden pins that.
    let tx2 = received(2, 11, &[Memo::Arbitrary(Box::new([7u8; 511]))]);
    // tx3: a plain send to Bob with a text memo.
    let tx3 = sent(3, 12, &bob, "hi Bob");
    // tx4: a sending transaction that also receives a text memo, which the
    // editorial layer renders as an extra memo-to-self value transfer.
    let txid4 = TxId::from_bytes([4; 32]);
    let tx4 = WalletTransaction::new_for_test_with_ironwood_notes(
        txid4,
        ConfirmationStatus::Confirmed(13.into()),
        vec![IronwoodNote::new_for_test(
            OutputId::new(txid4, 1),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default().build(),
            Memo::from_str("note to my future self").unwrap(),
            None,
        )],
        vec![OutgoingIronwoodNote::new_for_test(
            OutputId::new(txid4, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default().build(),
            Memo::from_str("second memo to Bob").unwrap(),
            Some(bob.clone()),
        )],
    );
    // tx5: a basic send-to-self (outgoing to one of the wallet's own
    // receivers, self-received internally).
    let txid5 = TxId::from_bytes([5; 32]);
    let tx5 = WalletTransaction::new_for_test_with_ironwood_notes(
        txid5,
        ConfirmationStatus::Confirmed(14.into()),
        vec![IronwoodNote::new_for_test(
            OutputId::new(txid5, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::Internal,
            OrchardCryptoNoteBuilder::default().build(),
            Memo::Empty,
            None,
        )],
        vec![OutgoingIronwoodNote::new_for_test(
            OutputId::new(txid5, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default()
                .recipient(own_orchard_receiver)
                .build(),
            Memo::Empty,
            None,
        )],
    );
    // tx6: the Zennies-for-Zingo pattern — a self-send output plus a send
    // to the ZFZ address in one transaction.
    let txid6 = TxId::from_bytes([6; 32]);
    let tx6 = WalletTransaction::new_for_test_with_ironwood_notes(
        txid6,
        ConfirmationStatus::Confirmed(15.into()),
        vec![],
        vec![
            OutgoingIronwoodNote::new_for_test(
                OutputId::new(txid6, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .recipient(own_orchard_receiver)
                    .build(),
                Memo::Empty,
                None,
            ),
            OutgoingIronwoodNote::new_for_test(
                OutputId::new(txid6, 1),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default().build(),
                Memo::Empty,
                Some(zfz_unified_address),
            ),
        ],
    );

    for transaction in [tx1, tx2, tx3, tx4, tx5, tx6] {
        alice_wallet
            .wallet_transactions
            .insert(transaction.txid(), transaction);
    }
    alice_wallet
}

fn bob_encoded() -> String {
    let network = ChainType::Regtest(ActivationHeights::default());
    let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, bob) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    bob.encode(&network)
}

/// Recursively sorts JSON object keys so HashMap-backed outputs compare
/// deterministically. Array order is preserved: it is part of the contract.
fn canonicalize(value: json::JsonValue) -> json::JsonValue {
    match value {
        json::JsonValue::Object(object) => {
            let mut entries: Vec<(String, json::JsonValue)> = object
                .iter()
                .map(|(key, value)| (key.to_string(), canonicalize(value.clone())))
                .collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            let mut sorted = json::object::Object::new();
            for (key, value) in entries {
                sorted.insert(&key, value);
            }
            json::JsonValue::Object(sorted)
        }
        json::JsonValue::Array(values) => {
            json::JsonValue::Array(values.into_iter().map(canonicalize).collect())
        }
        other => other,
    }
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn check_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("golden {} unreadable: {error}", path.display()));
    assert_eq!(
        actual, expected,
        "editorial output diverged from the pre-extraction golden {name}; \
         this is a consumer-contract break, not a fixture to update"
    );
}

#[tokio::test]
async fn editorial_surface_matches_goldens() {
    let wallet = editorial_wallet();
    let bob = bob_encoded();

    let value_transfers = wallet.value_transfers(true).await.unwrap();
    check_golden("value_transfers.txt", &value_transfers.to_string());
    check_golden(
        "value_transfers.json",
        &json::JsonValue::from(value_transfers).pretty(2),
    );
    check_golden(
        "value_transfers_ascending.json",
        &json::JsonValue::from(wallet.value_transfers(false).await.unwrap()).pretty(2),
    );

    check_golden(
        "messages_all.json",
        &json::JsonValue::from(wallet.messages_containing(None).await.unwrap()).pretty(2),
    );
    check_golden(
        "messages_bob.json",
        &json::JsonValue::from(wallet.messages_containing(Some(&bob)).await.unwrap()).pretty(2),
    );

    check_golden(
        "total_memobytes_to_address.json",
        &canonicalize(json::JsonValue::from(
            wallet.do_total_memobytes_to_address().await.unwrap(),
        ))
        .pretty(2),
    );
    check_golden(
        "total_value_to_address.json",
        &canonicalize(json::JsonValue::from(
            wallet.do_total_value_to_address().await.unwrap(),
        ))
        .pretty(2),
    );
    check_golden(
        "total_spends_to_address.json",
        &canonicalize(json::JsonValue::from(
            wallet.do_total_spends_to_address().await.unwrap(),
        ))
        .pretty(2),
    );
}
