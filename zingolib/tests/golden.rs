#![forbid(unsafe_code)]
#![cfg(all(feature = "perspective", feature = "testutils"))]
//! Golden regression harness pinning the editorial surface's JSON and
//! Display renderings byte-for-byte against pre-extraction captures,
//! canonicalizing key order only for the HashMap-backed `do_total_*`
//! rollups.

#[path = "support/common.rs"]
mod common;

use std::str::FromStr as _;

use pepper_sync::wallet::{IronwoodNote, OutgoingIronwoodNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::Memo;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingo_test_vectors::seeds;
use zingolib::config::ChainType;
use zingolib::mocks::orchard_note::OrchardCryptoNoteBuilder;
use zingolib::wallet::LightWallet;
use zingolib::wallet::keys::unified::ReceiverSelection;

use common::{own_orchard_receiver, received, regtest_wallet, self_send, sent, zfz_self_send};

/// A wallet exercising every editorial classification the goldens pin
/// (received text/empty/arbitrary memos, a plain send with a memo, a
/// memo-to-self on a sending transaction, a basic send-to-self, and the
/// Zennies-for-Zingo dual-output self-send), returned with Bob's encoded
/// address.
fn editorial_wallet() -> (LightWallet, String) {
    let mut alice_wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
    let network = ChainType::Regtest(ActivationHeights::default());

    let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, bob) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();

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
    let tx5 = self_send(5, 14, own_orchard_receiver(&alice_wallet));
    // tx6: the Zennies-for-Zingo pattern — a self-send output plus a send
    // to the ZFZ address in one transaction.
    let tx6 = zfz_self_send(6, 15, own_orchard_receiver(&alice_wallet));

    for transaction in [tx1, tx2, tx3, tx4, tx5, tx6] {
        alice_wallet
            .wallet_transactions
            .insert(transaction.txid(), transaction);
    }
    (alice_wallet, bob.encode(&network))
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
    let (wallet, bob) = editorial_wallet();

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
