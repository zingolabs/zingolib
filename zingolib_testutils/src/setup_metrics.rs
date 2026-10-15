//! Per-test instrumentation of chain-setup cost, feeding the chain-cache
//! migration (ADR 0003).
//!
//! Every libtonode test's `LocalNet` is wrapped in a [`MeteredNet`], which
//! samples the Validator's data-directory size at three points: right
//! after launch, at scenario-setup completion (the send boundary once
//! caches exist), and at teardown, plus the setup wall-clock, and appends
//! one JSON line per test to `chain_caches/setup-metrics.jsonl` at the
//! repository root. That location is bind-mounted into the test container,
//! so host tooling reads the numbers directly after a containerized run.
//!
//! Reading the samples: `launch_bytes` is the genesis-launch floor (the
//! chain-cache replay lands after launch, so cached and live runs are
//! distinguished by `setup_wall_ms`, not by launch size),
//! `setup_bytes - launch_bytes` is the chain setup established, and
//! `teardown_bytes - setup_bytes` is test-body growth, the measured
//! form of "does this test exercise mining behavior."

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::Instant;

use zcash_local_net::LocalNet;
use zcash_local_net::indexer::Indexer;
use zcash_local_net::validator::Validator;

use crate::observability::{FrontRecord, StateWatch, ZainodState, ZebradState};
use crate::scenarios::network_combo::{DefaultIndexer, DefaultValidator};
use zingolib::testutils::port_to_localhost_uri;

/// A `LocalNet` that records the chain-setup metrics of the test holding
/// it and carries the pipeline observatory (state watches on zebrad and
/// zainod, plus the front records connected before launch), writing its
/// metrics row when dropped. Derefs to the wrapped `LocalNet`, so call
/// sites use it exactly as before.
///
/// Since the front-proxy inversion, every port accessor on the wrapped
/// net returns an observing front: the zebrad front sees every
/// validator client (the launch-mine, the Indexer, the harness) and
/// the zainod front sees every wallet, so no hand-wired taps remain.
pub struct MeteredNet {
    net: LocalNet<DefaultValidator, DefaultIndexer>,
    recorder: SetupRecorder,
    zebrad_watch: StateWatch<ZebradState>,
    zainod_watch: StateWatch<ZainodState>,
    /// All validator clients: launch-mine, Indexer, harness.
    zebrad_front: std::sync::Arc<FrontRecord>,
    /// All Indexer clients: the wallets.
    zainod_front: std::sync::Arc<FrontRecord>,
}

impl MeteredNet {
    /// Wrap a freshly launched net: sample the launch-time data-dir
    /// size, prime the state watches, and adopt the front records that
    /// were connected before launch. `setup_started` is the instant
    /// scenario setup began, so the recorded wall-clock includes
    /// process launch.
    pub fn new(
        net: LocalNet<DefaultValidator, DefaultIndexer>,
        zebrad_front: std::sync::Arc<FrontRecord>,
        zainod_front: std::sync::Arc<FrontRecord>,
        setup_started: Instant,
    ) -> Self {
        let launch_bytes = dir_size(net.validator().data_dir().path());
        MeteredNet {
            zebrad_watch: StateWatch::prime(ZebradState {
                rpc_port: net.validator().rpc_listen_port(),
            }),
            zainod_watch: StateWatch::prime(ZainodState {
                uri: port_to_localhost_uri(net.indexer().listen_port()),
            }),
            zebrad_front,
            zainod_front,
            net,
            recorder: SetupRecorder {
                binary: current_binary_name(),
                test: current_test_name(),
                scenario: "custom_clients",
                launch_bytes,
                setup_bytes: None,
                setup_wall_ms: None,
                setup_started,
            },
        }
    }

    /// The Validator's chain-state timeline.
    pub fn zebrad_watch(&self) -> &StateWatch<ZebradState> {
        &self.zebrad_watch
    }

    /// The Indexer's indexed-tip timeline.
    pub fn zainod_watch(&self) -> &StateWatch<ZainodState> {
        &self.zainod_watch
    }

    /// The zebrad front's traffic record: every validator client.
    pub fn zebrad_front(&self) -> &FrontRecord {
        &self.zebrad_front
    }

    /// The zainod front's traffic record: every wallet.
    pub fn zainod_front(&self) -> &FrontRecord {
        &self.zainod_front
    }

    /// Record that scenario setup finished here. Nested constructors each
    /// call this on the way out. The outermost call runs last and wins, so
    /// the row reflects the scenario the test actually asked for.
    pub fn mark_setup_complete(&mut self, scenario: &'static str) {
        self.recorder.scenario = scenario;
        self.recorder.setup_bytes = Some(dir_size(self.net.validator().data_dir().path()));
        self.recorder.setup_wall_ms =
            Some(self.recorder.setup_started.elapsed().as_millis() as u64);
    }
}

impl Deref for MeteredNet {
    type Target = LocalNet<DefaultValidator, DefaultIndexer>;

    fn deref(&self) -> &Self::Target {
        &self.net
    }
}

impl DerefMut for MeteredNet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.net
    }
}

impl Drop for MeteredNet {
    fn drop(&mut self) {
        let teardown_bytes = dir_size(self.net.validator().data_dir().path());
        self.recorder.write_row(teardown_bytes);
        self.write_observatory_log();
    }
}

impl MeteredNet {
    /// Write the full observatory record (watch timelines, tap
    /// traffic, and the RPC ledger) to the per-test log under
    /// `test-logs/observatory/`, leaving a single pointer line on
    /// stderr. The location is gitignored and bind-mounted into the
    /// test container, so host tooling reads (and scp reaches) the
    /// logs directly. Best-effort: runs in `Drop`, possibly during a
    /// panic unwind, and must never mask the test's own outcome.
    fn write_observatory_log(&self) {
        let ledger = crate::validator_rpc::ledger_snapshot()
            .iter()
            .map(|entry| {
                format!(
                    "  {:>8.3}s {}",
                    entry
                        .at
                        .duration_since(self.recorder.setup_started)
                        .as_secs_f64(),
                    entry.method
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let report = format!(
            "==== observatory record: {}::{} ====\n\
             -- summaries --\n  {}\n  {}\n  {}\n  {}\n\
             -- zebrad timeline --\n{}\n\
             -- zainod timeline --\n{}\n\
             -- zebrad front (all validator clients: launch-mine, indexer, harness) --\n{}\n\
             -- zainod front (wallet clients) --\n{}\n\
             -- rpc ledger (this crate's outgoing calls) --\n{}\n",
            self.recorder.binary,
            self.recorder.test,
            self.zebrad_watch.summary(),
            self.zainod_watch.summary(),
            self.zebrad_front.summary(),
            self.zainod_front.summary(),
            self.zebrad_watch.render(),
            self.zainod_watch.render(),
            self.zebrad_front.render(),
            self.zainod_front.render(),
            ledger,
        );
        let path = observatory_log_path(&self.recorder.binary, &self.recorder.test);
        let written = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                file.write_all(report.as_bytes())
            });
        match written {
            Ok(()) => eprintln!("observatory log: {}", path.display()),
            Err(e) => eprintln!("observatory log unwritable ({}): {e}", path.display()),
        }
    }
}

struct SetupRecorder {
    binary: String,
    test: String,
    scenario: &'static str,
    launch_bytes: u64,
    setup_bytes: Option<u64>,
    setup_wall_ms: Option<u64>,
    setup_started: Instant,
}

impl SetupRecorder {
    /// Append this test's row. Runs in `Drop`, possibly during a panic
    /// unwind, so failures are reported to stderr rather than panicking:
    /// a lost metrics row must never mask the test's own outcome.
    fn write_row(&self, teardown_bytes: u64) {
        let row = serde_json::json!({
            "binary": self.binary,
            "test": self.test,
            "scenario": self.scenario,
            "launch_bytes": self.launch_bytes,
            "setup_bytes": self.setup_bytes,
            "teardown_bytes": teardown_bytes,
            "setup_wall_ms": self.setup_wall_ms,
        });
        let path = metrics_path();
        let appended = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                writeln!(file, "{row}")
            });
        if let Err(e) = appended {
            eprintln!("failed to append setup metrics to {}: {e}", path.display());
        }
    }
}

/// The gitignored cache-and-metrics root at the repository top level.
/// It survives `cargo clean` and reaches the host through the test
/// container's repo bind-mount (the container's `target/` is a named
/// volume the host cannot read).
pub(crate) fn chain_caches_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("zingolib_testutils sits directly under the repo root")
        .join("chain_caches")
}

fn metrics_path() -> PathBuf {
    chain_caches_root().join("setup-metrics.jsonl")
}

/// The per-test observatory log: `test-logs/observatory/<binary>__<test>.log`
/// at the repository root, gitignored, bind-mounted, appended per net
/// so a test that builds several nets keeps every record.
fn observatory_log_path(binary: &str, test: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("zingolib_testutils sits directly under the repo root")
        .join("test-logs")
        .join("observatory")
        .join(format!("{binary}__{}.log", test.replace("::", "__")))
}

/// The integration-test binary this test runs in, from the executable's
/// file stem with cargo's trailing `-<hash>` disambiguator stripped. The
/// thread name alone is ambiguous: it is the test's path *within* its
/// binary, and libtonode has several test binaries whose module paths
/// can collide.
pub(crate) fn current_binary_name() -> String {
    let Some(stem) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
    else {
        return "unknown".to_string();
    };
    match stem.rsplit_once('-') {
        Some((name, hash)) if !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
            name.to_string()
        }
        _ => stem,
    }
}

/// The test's path, from the thread name the libtest harness assigns.
/// Scenario constructors are awaited in the test's root future, which
/// tokio polls on the test thread under both runtime flavors, but a
/// constructor awaited inside a spawned task would land on a worker
/// thread, so fail loudly rather than key metrics (and later, chain
/// caches) to a garbage name.
pub(crate) fn current_test_name() -> String {
    let thread = std::thread::current();
    let name = thread
        .name()
        .expect("test threads are named by the harness");
    assert!(
        !name.starts_with("tokio-runtime-worker"),
        "scenario setup must run in the test's root future, not a spawned task; \
         found thread {name:?}"
    );
    name.to_string()
}

/// Total size in bytes of all files under `path`, without following
/// symlinks. Unreadable entries count as zero: this feeds metrics, and a
/// permissions hiccup must never fail a test.
fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}
