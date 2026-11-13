use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use proptest::prelude::*;
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;
use zaino_proto::tonic::Code;
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_netutils::time::{MEMPOOL_DRAIN_CEILING, MEMPOOL_DRAIN_SETTLE};

use super::*;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::mock_indexer::{Fault, Rpc};
use crate::testutils::synthetic_wallet;
use pepper_sync::wallet::OutgoingNoteInterface as _;

const CASE_FUNDING: u64 = 300_000;
const FETCH_FAILURES: usize = 2;
const SENDS: [u64; 3] = [1_000, 10_000, 50_000];
const RECEIPTS: [u64; 1] = [30_000];
const GENERATED_SESSIONS: u32 = 4;
const SHRINK_LIMIT: u32 = 16;
const SYNC_STALL_LIMIT: Duration = Duration::from_secs(120);
const ENGINE_LOG_CAPACITY: usize = 4_096;
const ENGINE_LOG_FILTER: &str = "pepper_sync=debug,zingolib=debug";
const LATE_ANSWER: Duration = Duration::from_millis(1_000);
const STREAM_AFTER_DRAIN_CEILING: Duration = MEMPOOL_DRAIN_CEILING.saturating_add(LATE_ANSWER);
const STALLED_SESSION: [Step; 14] = [
    Step::Send(10_000),
    Step::Sync,
    Step::MineMempool,
    Step::Send(50_000),
    Step::Send(50_000),
    Step::Send(50_000),
    Step::MineMempool,
    Step::MineMempool,
    Step::Sync,
    Step::MineMempool,
    Step::MineMempool,
    Step::Send(1_000),
    Step::MineMempool,
    Step::Receive(30_000),
];

static ENGINE_LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

struct EngineLogLine(Vec<u8>);

impl io::Write for EngineLogLine {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Drop for EngineLogLine {
    fn drop(&mut self) {
        note_in_engine_log(String::from_utf8_lossy(&self.0).trim_end().to_string());
    }
}

fn note_in_engine_log(line: String) {
    let mut lines = ENGINE_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if lines.len() == ENGINE_LOG_CAPACITY {
        lines.pop_front();
    }
    lines.push_back(line);
}

fn capture_engine_log() -> bool {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(ENGINE_LOG_FILTER))
        .with_ansi(false)
        .with_writer(|| EngineLogLine(Vec::new()))
        .try_init()
        .is_ok()
}

fn session_runtime() -> Runtime {
    Runtime::new().expect("the test host grants the threads of a tokio runtime")
}

enum SessionEnd {
    Completed,
    Errored(String),
    Stalled(String),
}

async fn sync_session(client: &mut LightClient) -> SessionEnd {
    match tokio::time::timeout(SYNC_STALL_LIMIT, client.sync_and_await()).await {
        Ok(Ok(_sync_result)) => SessionEnd::Completed,
        Ok(Err(error)) => SessionEnd::Errored(error.to_string()),
        Err(_elapsed) => {
            let report = stall_report(client);
            eprintln!("{report}");
            SessionEnd::Stalled(report)
        }
    }
}

fn engine_log_tail() -> String {
    let lines = ENGINE_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    let log = lines
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "the last {} log lines of the engine and the session:\n{log}",
        lines.len()
    )
}

fn stall_report(client: &LightClient) -> String {
    format!(
        "SYNC STALL: the Sync Session did not return within {SYNC_STALL_LIMIT:?}\n\
         sync mode: {:?}\n\
         sync status: {:?}\n\
         {}",
        client.sync_mode(),
        client.latest_sync_status(),
        engine_log_tail(),
    )
}

fn setup_stall_report() -> String {
    let report = format!(
        "SYNC STALL: the funding of the wallet did not return within {SYNC_STALL_LIMIT:?}\n{}",
        engine_log_tail(),
    );
    eprintln!("{report}");
    report
}

async fn delay_next_calls(net: &MockNet, delays: &[(Rpc, Duration)]) {
    let mut chain = net.chain.write().await;
    for (rpc, delay) in delays {
        chain.faults.clear(*rpc);
        chain.faults.inject(*rpc, Fault::Delay(*delay));
    }
}

fn replay_stalled_session(delays: &[(Rpc, Duration)]) {
    let _capturing = capture_engine_log();
    if let Err(report) = session_runtime().block_on(run_session(&STALLED_SESSION, delays)) {
        panic!("{report}");
    }
}

#[test]
fn the_stalled_session_returns_with_no_late_answer() {
    replay_stalled_session(&[]);
}

#[test]
fn the_stalled_session_returns_when_the_mempool_stream_connects_inside_the_drain_ceiling() {
    replay_stalled_session(&[(Rpc::MempoolStream, MEMPOOL_DRAIN_SETTLE)]);
}

#[test]
fn the_stalled_session_returns_when_the_mempool_stream_connects_after_the_drain_ceiling() {
    replay_stalled_session(&[(Rpc::MempoolStream, STREAM_AFTER_DRAIN_CEILING)]);
}

#[test]
fn the_stalled_session_returns_when_the_chain_height_answers_late() {
    replay_stalled_session(&[(Rpc::LatestBlock, LATE_ANSWER)]);
}

#[test]
fn the_stalled_session_returns_when_the_block_range_answers_late() {
    replay_stalled_session(&[(Rpc::BlockRange, LATE_ANSWER)]);
}

#[test]
fn the_stalled_session_returns_when_the_compact_block_answers_late() {
    replay_stalled_session(&[(Rpc::Block, LATE_ANSWER)]);
}

#[test]
fn the_stalled_session_returns_when_the_tree_state_answers_late() {
    replay_stalled_session(&[(Rpc::TreeState, LATE_ANSWER)]);
}

#[test]
fn the_stalled_session_returns_when_the_subtree_roots_answer_late() {
    replay_stalled_session(&[(Rpc::SubtreeRoots, LATE_ANSWER)]);
}

#[test]
fn the_stalled_session_returns_when_every_scan_call_answers_late() {
    replay_stalled_session(&[
        (Rpc::LatestBlock, LATE_ANSWER),
        (Rpc::BlockRange, LATE_ANSWER),
        (Rpc::Block, LATE_ANSWER),
        (Rpc::TreeState, LATE_ANSWER),
        (Rpc::SubtreeRoots, LATE_ANSWER),
    ]);
}

#[derive(Debug, Clone)]
enum Step {
    Sync,
    Send(u64),
    Receive(u64),
    MineMempool,
    MineEmpty(u32),
    WithholdFetches(bool),
}

fn any_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::Sync),
        3 => (0..SENDS.len()).prop_map(|index| Step::Send(SENDS[index])),
        1 => (0..RECEIPTS.len()).prop_map(|index| Step::Receive(RECEIPTS[index])),
        3 => Just(Step::MineMempool),
        2 => (0u32..4).prop_map(Step::MineEmpty),
        1 => proptest::bool::ANY.prop_map(Step::WithholdFetches),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: GENERATED_SESSIONS,
        max_shrink_iters: SHRINK_LIMIT,
        ..ProptestConfig::default()
    })]
    #[test]
    fn every_step_of_a_generated_session_holds_the_wallet_invariants(
        steps in prop::collection::vec(any_step(), 1..16),
    ) {
        let _capturing = capture_engine_log();
        session_runtime()
            .block_on(run_session(&steps, &[]))
            .map_err(|failure| TestCaseError::Fail(failure.into()))?;
    }
}

async fn run_session(steps: &[Step], delays: &[(Rpc, Duration)]) -> Result<(), String> {
    let mut net = MockNet::launch().await;
    note_in_engine_log("SESSION STEP funding".to_string());
    let funding = funded_client(&mut net, &[(PoolType::ORCHARD, CASE_FUNDING)], 1);
    let Ok(mut client) = tokio::time::timeout(SYNC_STALL_LIMIT, funding).await else {
        return Err(setup_stall_report());
    };
    let mut issued = CASE_FUNDING;

    for (index, step) in steps.iter().enumerate() {
        note_in_engine_log(format!("SESSION STEP {index}: {step:?}"));
        match step {
            Step::Sync => (),
            Step::Send(value) => {
                let _ = from_inputs::quick_send(
                    &mut client,
                    vec![(
                        &synthetic_wallet::external_address(PoolType::ORCHARD),
                        *value,
                        None,
                    )],
                )
                .await;
            }
            Step::Receive(value) => {
                let address =
                    get_base_address(&client, PoolType::Shielded(ShieldedPool::Ironwood)).await;
                let funding = net
                    .funding_transaction(vec![(&address, *value, None)])
                    .await;
                net.chain.write().await.enter_mempool(funding);
                issued += value;
            }
            Step::MineMempool => net.chain.write().await.mine_mempool(),
            Step::MineEmpty(count) => net.chain.write().await.mine_empty_blocks(*count),
            Step::WithholdFetches(withhold) => {
                let mut chain = net.chain.write().await;
                if *withhold {
                    for _ in 0..FETCH_FAILURES {
                        chain.faults.inject(
                            Rpc::BlockRange,
                            Fault::Fail(Code::Unavailable, "mock outage".to_string()),
                        );
                    }
                } else {
                    chain.faults.clear(Rpc::BlockRange);
                }
            }
        }
        delay_next_calls(&net, delays).await;
        if let SessionEnd::Stalled(report) = sync_session(&mut client).await {
            return Err(format!("after step {index}, {step:?}: {report}"));
        }
        check(&client, issued)
            .await
            .map_err(|failure| format!("after {step:?}: {failure}"))?;
    }

    net.chain.write().await.faults.clear(Rpc::BlockRange);
    net.chain.write().await.mine_mempool();
    net.chain.write().await.mine_empty_blocks(1);
    note_in_engine_log("SESSION STEP settlement".to_string());
    delay_next_calls(&net, delays).await;
    match sync_session(&mut client).await {
        SessionEnd::Completed => (),
        SessionEnd::Errored(error) => {
            return Err(format!("the settling session errors: {error}"));
        }
        SessionEnd::Stalled(report) => return Err(format!("at settlement: {report}")),
    }
    check_settled(&client, issued)
        .await
        .map_err(|failure| format!("at settlement: {failure}"))
}

/// The invariants hold and the wallet holds no more value than was issued.
async fn check(client: &LightClient, issued: u64) -> Result<(), String> {
    let wallet = client.wallet().read().await;
    let violations = wallet.invariant_violations();
    if !violations.is_empty() {
        return Err(crate::wallet::invariants::violation_report(&violations));
    }
    let balance = wallet
        .account_balance(zip32::AccountId::ZERO)
        .map_err(|error| format!("account_balance errored: {error}"))?;
    let held = balance
        .total_ironwood_balance
        .expect("an ironwood-capable wallet reports an ironwood balance")
        .into_u64();
    if held > issued {
        return Err(format!(
            "the wallet holds {held}, more than the {issued} issued"
        ));
    }
    Ok(())
}

/// With every chain side settled, the held value equals funding plus receipts
/// minus what the sent transactions took.
async fn check_settled(client: &LightClient, issued: u64) -> Result<(), String> {
    let wallet = client.wallet().read().await;
    let violations = wallet.invariant_violations();
    if !violations.is_empty() {
        return Err(crate::wallet::invariants::violation_report(&violations));
    }
    let mut expected = issued;
    for transaction in wallet.wallet_transactions.values() {
        if transaction.status().is_failed() {
            continue;
        }
        let payment: u64 = transaction
            .outgoing_orchard_notes()
            .iter()
            .map(|note| note.value())
            .sum::<u64>()
            + transaction
                .outgoing_sapling_notes()
                .iter()
                .map(|note| note.value())
                .sum::<u64>()
            + transaction
                .outgoing_ironwood_notes()
                .iter()
                .map(|note| note.value())
                .sum::<u64>();
        if payment == 0 {
            continue;
        }
        let fee = wallet
            .calculate_transaction_fee(transaction)
            .ok()
            .map(zcash_protocol::value::Zatoshis::into_u64)
            .unwrap_or(0);
        expected = expected.saturating_sub(payment + fee);
    }
    let balance = wallet
        .account_balance(zip32::AccountId::ZERO)
        .map_err(|error| format!("account_balance errored: {error}"))?;
    let held = balance
        .total_ironwood_balance
        .expect("an ironwood-capable wallet reports an ironwood balance")
        .into_u64();
    if held != expected {
        return Err(format!(
            "the wallet holds {held}, expected {expected} of the {issued} issued"
        ));
    }
    Ok(())
}
