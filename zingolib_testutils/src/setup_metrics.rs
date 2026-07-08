//! Per-test instrumentation of chain-setup cost, feeding the chain-cache
//! migration (ADR 0003).
//!
//! Every libtonode test's `LocalNet` is wrapped in a [`MeteredNet`], which
//! samples the Validator's data-directory size at three points — right
//! after launch, at scenario-setup completion (the send boundary once
//! caches exist), and at teardown — plus the setup wall-clock, and appends
//! one JSON line per test to `chain_caches/setup-metrics.jsonl` at the
//! repository root. That location is bind-mounted into the test container,
//! so host tooling reads the numbers directly after a containerized run.
//!
//! Reading the samples: `launch_bytes` is the genesis-launch floor (the
//! chain-cache replay lands after launch, so cached and live runs are
//! distinguished by `setup_wall_ms`, not by launch size),
//! `setup_bytes - launch_bytes` is the chain setup established, and
//! `teardown_bytes - setup_bytes` is test-body growth — the measured
//! form of "does this test exercise mining behavior."

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::Instant;

use zcash_local_net::LocalNet;
use zcash_local_net::validator::Validator;

use crate::scenarios::network_combo::{DefaultIndexer, DefaultValidator};

/// A `LocalNet` that records the chain-setup metrics of the test holding
/// it, writing its row when dropped. Derefs to the wrapped `LocalNet`, so
/// call sites use it exactly as before.
pub struct MeteredNet {
    net: LocalNet<DefaultValidator, DefaultIndexer>,
    recorder: SetupRecorder,
}

impl MeteredNet {
    /// Wrap a freshly launched net, sampling the launch-time data-dir size.
    /// `setup_started` is the instant scenario setup began, so the recorded
    /// wall-clock includes process launch.
    pub fn new(net: LocalNet<DefaultValidator, DefaultIndexer>, setup_started: Instant) -> Self {
        let launch_bytes = dir_size(net.validator().data_dir().path());
        MeteredNet {
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

    /// Record that scenario setup finished here. Nested constructors each
    /// call this on the way out; the outermost call runs last and wins, so
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
    /// unwind, so failures are reported to stderr rather than panicking —
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
/// tokio polls on the test thread under both runtime flavors — but a
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
