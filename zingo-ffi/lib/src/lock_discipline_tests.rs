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
/// guard on `LIGHTCLIENT`, and returns its outcome. Panics only if the
/// endpoint blocks behind the guard (it takes the write lock).
fn outcome_under_held_read_lock(
    endpoint: impl FnOnce() -> Result<String, ZingolibError> + Send + 'static,
) -> Result<String, ZingolibError> {
    let _reader = LIGHTCLIENT
        .read()
        .expect("no serialized test leaves the lock poisoned");
    let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = outcome_tx.send(endpoint());
    });
    outcome_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the endpoint queued behind a held read guard: it takes the write lock")
}

/// [`outcome_under_held_read_lock`] for the common case: the endpoint must
/// answer Ok with well-formed JSON.
fn answer_under_held_read_lock(
    endpoint: impl FnOnce() -> Result<String, ZingolibError> + Send + 'static,
) -> json::JsonValue {
    let answer = outcome_under_held_read_lock(endpoint)
        .expect("the initialized wallet answers this endpoint");
    json::parse(&answer).expect("the endpoint answers well-formed JSON")
}

#[test]
fn value_transfers_answer_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_value_transfers);
    // A fresh wallet has no history, and the empty list still arrives
    // under its named key.
    assert!(
        answer["value_transfers"].is_array() && answer["value_transfers"].is_empty(),
        "the fixture wallet's history is an empty list: {answer}"
    );
}

#[test]
fn status_sync_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(status_sync);
    // A never-synced wallet reports an empty scan plan with zero progress.
    assert!(
        answer["scan_ranges"].is_array() && answer["scan_ranges"].is_empty(),
        "no scan ranges before a first sync: {answer}"
    );
    assert_eq!(
        answer["total_outputs_scanned"].as_u64(),
        Some(0),
        "nothing scanned before a first sync: {answer}"
    );
}

#[test]
fn seed_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_seed);
    // The NewSeed fixture carries a 24-word mnemonic, its Library Birthday,
    // and the wallet's own chain.
    assert_eq!(
        answer["seed_phrase"]
            .as_str()
            .map(|phrase| phrase.split_whitespace().count()),
        Some(24),
        "the recovery info carries the 24-word mnemonic: {answer}"
    );
    assert!(
        answer["birthday"].as_u32().is_some_and(|height| height > 0),
        "an offline new wallet gets the positive Library Birthday: {answer}"
    );
    assert_eq!(answer["chain_name"].as_str(), Some("main"), "{answer}");
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
fn config_wallet_performance_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_config_wallet_performance);
    // The fixture initializes with the Medium performance level.
    assert_eq!(
        answer["performance_level"].as_str(),
        Some("Medium"),
        "the fixture wallet's configured level: {answer}"
    );
}

#[test]
fn wallet_version_answers_beside_a_held_read_guard() {
    let _serial = serialized();
    init_offline_wallet();
    let answer = answer_under_held_read_lock(get_wallet_version);
    // A freshly created wallet reads back at the version it was written
    // with, so the two versions agree and are positive.
    let current = answer["current_version"].as_u32();
    assert!(
        current.is_some_and(|version| version > 0),
        "the wallet has a positive serialization version: {answer}"
    );
    assert_eq!(
        answer["read_version"].as_u32(),
        current,
        "a fresh wallet's read version matches current: {answer}"
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
