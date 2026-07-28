//! Lock-discipline tests for the read-only FFI endpoints (zingo-mobile#1223).
//!
//! Each test initializes the offline wallet, then calls its endpoint from
//! another thread while the test thread holds a read guard on `LIGHTCLIENT`.
//! An endpoint on the read lock answers concurrently; one that takes the
//! write lock queues behind the held guard and times out. The answer is then
//! checked against the fixture wallet, so a port that silences the endpoint
//! fails just as loudly as one that keeps the write lock.

use super::*;

/// Serializes the tests that initialize or lock the global `LIGHTCLIENT`.
/// nextest isolates each test in its own process; this guard keeps the
/// suite correct under plain `cargo test`'s in-process threads too.
static LIGHTCLIENT_SERIAL: Mutex<()> = Mutex::new(());

pub(crate) fn serialized() -> std::sync::MutexGuard<'static, ()> {
    LIGHTCLIENT_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Builds a fresh Indexerless mainnet wallet (the offline `init_new` path)
/// and stores it as the global client.
pub(crate) fn init_offline_wallet() {
    init_new(
        String::new(),
        0,
        "main".to_string(),
        "Medium".to_string(),
        1,
    )
    .expect("the offline Indexerless wallet must initialize");
}

/// Runs `endpoint` on another thread while the caller's thread holds a read
/// guard on `LIGHTCLIENT`, and returns its parsed answer. Panics if the
/// endpoint blocks behind the guard (it takes the write lock), errors, or
/// answers with malformed JSON.
fn answer_under_held_read_lock(endpoint: fn() -> Result<String, ZingolibError>) -> json::JsonValue {
    let _reader = LIGHTCLIENT
        .read()
        .expect("no serialized test leaves the lock poisoned");
    let (answer_tx, answer_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = answer_tx.send(endpoint());
    });
    let answer = answer_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the endpoint queued behind a held read guard: it takes the write lock")
        .expect("the initialized wallet answers this endpoint");
    json::parse(&answer).expect("the endpoint answers well-formed JSON")
}

#[test]
fn wallet_kind_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(wallet_kind);
    // The offline fixture is a NewSeed wallet, so its kind is the mnemonic
    // one with every receiver present.
    assert_eq!(
        answer["kind"].as_str(),
        Some("Loaded from seed or mnemonic phrase"),
        "the fixture wallet's kind answer changed shape: {answer}"
    );
    for receiver in ["transparent", "sapling", "orchard"] {
        assert_eq!(
            answer[receiver].as_bool(),
            Some(true),
            "a seed wallet carries every receiver: {answer}"
        );
    }
}

#[test]
fn unified_addresses_answer_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_unified_addresses);
    // A fresh wallet derives exactly one unified address, Orchard-only,
    // for account 0.
    assert_eq!(answer.len(), 1, "one derived address: {answer}");
    let address = &answer[0];
    assert_eq!(address["account"].as_u32(), Some(0), "{answer}");
    assert_eq!(address["has_orchard"].as_bool(), Some(true), "{answer}");
    assert!(
        address["encoded_address"]
            .as_str()
            .is_some_and(|encoded| encoded.starts_with("u1")),
        "a mainnet unified address encodes with the u1 prefix: {answer}"
    );
}

#[test]
fn transparent_addresses_answer_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_transparent_addresses);
    // A fresh wallet derives exactly one external transparent address for
    // account 0.
    assert_eq!(answer.len(), 1, "one derived address: {answer}");
    let address = &answer[0];
    assert_eq!(address["account"].as_u32(), Some(0), "{answer}");
    assert_eq!(address["scope"].as_str(), Some("external"), "{answer}");
    assert!(
        address["encoded_address"]
            .as_str()
            .is_some_and(|encoded| encoded.starts_with("t1")),
        "a mainnet P2PKH address encodes with the t1 prefix: {answer}"
    );
}

#[test]
fn wallet_save_required_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_wallet_save_required);
    // A freshly created wallet has never been saved, so a save is required.
    assert_eq!(
        answer["save_required"].as_bool(),
        Some(true),
        "the fresh fixture wallet requires a save: {answer}"
    );
}

#[test]
fn latest_block_wallet_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_latest_block_wallet);
    // A fresh offline wallet has no last known chain height, which this
    // endpoint reports as 0.
    assert_eq!(
        answer["height"].as_u32(),
        Some(0),
        "the fixture wallet's height answer changed shape: {answer}"
    );
}
