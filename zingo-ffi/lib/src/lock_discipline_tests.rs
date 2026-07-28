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

fn serialized() -> std::sync::MutexGuard<'static, ()> {
    LIGHTCLIENT_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Builds a fresh Indexerless mainnet wallet (the offline `init_new` path)
/// and stores it as the global client.
fn init_offline_wallet() {
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
fn answer_under_held_read_lock(
    endpoint: fn() -> Result<String, ZingolibError>,
) -> json::JsonValue {
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
