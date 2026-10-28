//! Compares two arms' sync rate inside a real CLI session, interleaved,
//! with everything but the arm held fixed.
//!
//! A benchmark that varies more than one thing measures nothing. This holds
//! the indexer, the wallet seed, the scanned window, the performance level,
//! and the build profile constant, gives each arm its own worktree so
//! neither meets the other's build artifacts, runs the two alternately so
//! warm-up and machine load fall on both equally, and reports outputs per
//! second, which is the rate scan cost actually tracks.
//!
//! An arm is a commit and a build kind, written `<commit>[:nakednet]`. The
//! default kind builds the CLI mixnet-capable and bundles the proxy, so an
//! Online session spawns it, proves the quartet, and runs the
//! Server-Selection Sweep beside the scan; `:nakednet` builds without the
//! mixnet and bundles nothing. Sync itself rides nakednet in both kinds,
//! since the sync engine takes a plain gRPC indexer and no SOCKS5 route, so
//! one commit against itself in the two kinds measures what the mixnet's
//! presence costs a nakednet sync rather than what a mixnet carrier costs.
//!
//! The reading comes from the sync engine's own clock. The tool grafts a
//! span into each arm's checkout rather than requiring the arm to carry
//! one, so the arms measure their own code and not their own harnesses,
//! and so a comparison reaches back past the commit that instrumented the
//! library. Boot is wall clock by necessity, spanning the spawn to the
//! span's opening, and it carries the build and the bootstrap.
//!
//! The window is the caller's: `--birthday` is required and every arm
//! receives the one value, so neither this tool nor the arms ever ask a
//! moving tip where to start. A discarded warm-up per arm still runs, since
//! a first run pays for the build and reads slower than its successors.
//!
//! A run may be bounded rather than run to completion. `--seconds <n>`
//! stops the scan that long after its span opens, and the engine answers a
//! stop with the session's own counts, so the rate means what a whole
//! scan's would. That is what makes a range of hundreds of thousands of
//! blocks affordable to measure, and it measures the early phase a user
//! actually waits through.
//!
//! A run to completion passes `--waitsync`, which sets the session's sync
//! to shut down once the wallet is up to date, so the span closes on its
//! own. A continuous session would go on scanning each new block and
//! never return. A bounded run leaves the sync continuous and stops it,
//! since a session waiting for its sync reads no command until then.
//!
//! The wallet is the caller's too. `--seed` restores a named mnemonic in
//! every arm, defaulting to a fundless one. A wallet with history decrypts
//! notes where a fundless one only discards, so reproducing someone's
//! measurement means giving the comparison their wallet.
//!
//! Two readings ride beside the rate. The session's peak resident memory
//! is the CLI process's high-water mark, read from `/proc` once the span
//! closes and before the session quits, so it spans the boot and the
//! whole scan and leaves out the proxy, which runs as a process of its
//! own. The wallet file's size is read after the session quits, so it
//! carries the save the shutdown wrote. A platform without `/proc`
//! reports the memory as unavailable and the rest of the comparison
//! stands. Both readings grow with the span scanned, so a bounded run
//! compares them only as far as its arms scanned alike, and a comparison
//! that means to measure them runs to completion.
//!
//! `--json <path>` writes the readings in Bencher Metric Format beside the
//! report, a benchmark per arm named by its role and the window. A sink
//! that graphs the nights then holds the baseline measured again each
//! night beside the candidate, and that baseline series is the runner's
//! own noise: a candidate that moves within it has not moved.
//!
//! Usage: `makers sync-ab [cli] <arm> <arm> --birthday <height>
//! [--runs <n>] [--seconds <n>] [--seed <mnemonic>]`. The reserved first word names the session
//! kind, as `makers test packages` and `makers test live` name their
//! scopes, and omitting it names the same kind.
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use workbench::session;
use workbench::{cargo_make, repo_root, run, write};

/// The session kind a reserved first word names, following the house idiom
/// that gives `makers test` its `packages` and `live` words.
// One kind exists here, and the word still earns its place: an in-process
// harness is a second kind, and naming this one now lets that one arrive as
// another word rather than as a redesign of the grammar.
const CLI_SESSION: &str = "cli";

/// The suffix marking an arm built without the mixnet.
const NAKEDNET_KIND: &str = ":nakednet";

/// The sync launch every era of this library shares, and the anchor the
/// graft wraps.
const SPAWN_ANCHOR: &str = "let sync_handle = tokio::spawn(async move {";

/// The call the wrapped text must contain, so a graft that found the wrong
/// block refuses instead of producing a number.
const SPAWN_CALL: &str = "pepper_sync::sync";

/// The file the graft rewrites in each arm's worktree.
const SYNC_SOURCE: &str = "zingolib/src/lightclient/sync.rs";

/// The marker the graft opens its span with.
const SYNC_SPAN_OPEN: &str = "SYNC_SPAN=open";

/// The marker the graft closes its span with, followed by the duration.
const SYNC_SPAN_CLOSE: &str = "SYNC_SPAN=close";

/// The key on the closing marker carrying the outputs the span scanned.
const SYNC_SPAN_OUTPUTS: &str = "outputs=";

// The word a span closes on when the sync failed.
const SPAN_FAILED: &str = "err";

// A sync that closes on an error may be relaunched by the CLI's own
// recovery, which opens a new span once the prompt redraws. The failure
// stands only after this long with no new span.
const RECOVERY_GRACE: Duration = Duration::from_secs(10);

/// The unit suffix the closing marker's duration carries.
const MILLIS_SUFFIX: &str = "ms";

/// The log filter the session runs under, since an empty one silences the
/// markers this reads.
const LOG_FILTER: &str = "info";

/// The BIP-39 mnemonic every arm restores, when `--seed` names no other.
// A fundless wallet measures pure scanning: every output is trial
// decrypted and discarded, and none is decrypted twice for its note. A
// wallet with history does more work per output, so a comparison that
// means to reproduce one must be given that wallet rather than this one.
const GUARD_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// Measured runs per arm, when `--runs` names no other count.
const DEFAULT_RUNS: usize = 3;

/// How long a run scans before it is stopped, when `--seconds` names no
/// other span. Zero lets the scan run to completion.
// A bounded run measures the rate over the window a user actually waits
// through, and it makes a long range affordable: the engine reports the
// session's own counts when it stops, so a partial scan yields the same
// rate a whole one would.
const DEFAULT_SECONDS: u64 = 0;

/// How long one measured session may take before it counts as failed.
const RUN_BUDGET: Duration = Duration::from_secs(900);

/// How long a warm-up may take, which is the run that pays for the build.
const WARMUP_BUDGET: Duration = Duration::from_secs(3_600);

/// How often the driver rereads the log for the markers.
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The degree of freedom a sample standard deviation gives up to its mean.
const BESSEL_CORRECTION: usize = 1;

// The directory under the scratch space the session keeps its wallet in.
const WALLETS_DIR: &str = "wallets";

// The one file the CLI writes its wallet to, restated from zingolib's
// `config::DEFAULT_WALLET_NAME` because the workbench is std-only; it must
// not drift from that constant.
const WALLET_FILE: &str = "zingo-wallet.dat";

// The process name the memory reading follows, as the kernel reports it.
const CLI_COMM: &str = "zingo-cli";

// Where the kernel publishes each process's name, children, and memory.
const PROC: &str = "/proc";

// The file under a process's `/proc` entry that names it.
const STAT_FILE: &str = "stat";

// The file under a process's `/proc` entry that carries its memory.
const STATUS_FILE: &str = "status";

// The directory under a process's `/proc` entry holding one entry per
// thread, and the file under each thread listing the processes it spawned.
const TASK_DIR: &str = "task";
const CHILDREN_FILE: &str = "children";

// The status line that carries the peak resident set, and its unit.
const HIGH_WATER_FIELD: &str = "VmHWM:";
const HIGH_WATER_UNIT: &str = "kB";

// The units the readings print in.
const KIB: u64 = 1024;
const MIB: u64 = KIB * KIB;
const KIB_NAME: &str = "KiB";
const MIB_NAME: &str = "MiB";

// The word for a reading the platform did not give.
const UNAVAILABLE: &str = "unavailable";

// The role each arm plays in the metrics file, by position: the first arm
// is the baseline the second is measured against.
const BASELINE_ROLE: &str = "baseline";
const CANDIDATE_ROLE: &str = "candidate";

// The measures the metrics file carries, each named with its unit so a
// sink that creates a measure from its slug needs no further description.
const OUTPUTS_PER_SECOND: &str = "outputs-per-second";
const BOOT_SECONDS: &str = "boot-seconds";
const PEAK_RSS_BYTES: &str = "peak-rss-bytes";
const WALLET_FILE_BYTES: &str = "wallet-file-bytes";

// The words a benchmark's window is written in.
const TO_COMPLETION: &str = "to completion";
const SEEDED: &str = ", seeded";

// The factor from a ratio to a percentage.
const PERCENT: f64 = 100.0;

/// How far the arms' output counts may differ, as a percentage, before the
/// comparison is reported as drifted: the tip advances during a run, so a
/// later run scans a few more outputs than an earlier one.
const OUTPUT_DRIFT_TOLERANCE: f64 = 1.0;

/// How far a bounded run's durations may differ, as a percentage, before
/// the comparison is reported as drifted.
// A stop lands after the batch in flight rather than on the instant it is
// asked for, so a bounded span overshoots by up to one batch and the
// spread a fixed window allows would fire on that alone.
const SPAN_DRIFT_TOLERANCE: f64 = 10.0;

fn main() {
    run("sync-ab", compare, |()| {})
}

/// A commit and the build kind it is measured in.
#[derive(Clone, PartialEq, Eq)]
struct Spec {
    commit: String,
    nakednet: bool,
}

impl Spec {
    /// Reads `<commit>[:nakednet]`.
    fn parse(raw: &str) -> Self {
        match raw.strip_suffix(NAKEDNET_KIND) {
            Some(commit) => Spec {
                commit: commit.to_string(),
                nakednet: true,
            },
            None => Spec {
                commit: raw.to_string(),
                nakednet: false,
            },
        }
    }

    /// The arm's label, short enough to head a row.
    fn label(&self) -> String {
        let commit: String = self.commit.chars().take(12).collect();
        if self.nakednet {
            format!("{commit}{NAKEDNET_KIND}")
        } else {
            commit
        }
    }

    /// The arm rendered as one path component, since a branch name may
    /// carry a slash and a directory name may not.
    fn slug(&self) -> String {
        self.label()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }
}

/// What the invocation asked for.
struct Request {
    specs: [Spec; 2],
    birthday: u32,
    runs: usize,
    seconds: u64,
    seed: String,
    // Where the readings go in Bencher Metric Format, when a sink wants
    // them.
    json: Option<PathBuf>,
}

/// One arm's checkout, how it is launched, and its readings.
struct Arm {
    spec: Spec,
    worktree: PathBuf,
    /// Whether the arm's `Makefile.toml` carries the `run-cli` task, which
    /// builds mixnet-capable and bundles the proxy. An arm predating it is
    /// built and launched directly.
    hosted: bool,
    /// Whether the arm's CLI knows the consent flag that takes a session
    /// online. An arm without one is driven by `sync run` instead.
    consents: bool,
    rates: Vec<f64>,
    outputs: Vec<u64>,
    boots: Vec<Duration>,
    syncs: Vec<Duration>,
    // The CLI's peak resident set and the wallet file's size per session,
    // in bytes, with a session that gave no reading kept as `None` so the
    // arms are never averaged over different sessions.
    peaks: Vec<Option<u64>>,
    wallet_files: Vec<Option<u64>>,
}

/// One session's reading, taken from the engine's span but for the boot.
struct Reading {
    boot: Duration,
    sync: Duration,
    outputs: u64,
    // The CLI's peak resident set in bytes, when the platform publishes it,
    // and the wallet file's size in bytes once the session has quit.
    peak_rss: Option<u64>,
    wallet_file: Option<u64>,
}

impl Reading {
    /// Outputs per second, which is the rate scan cost tracks.
    fn rate(&self) -> f64 {
        let seconds = self.sync.as_secs_f64();
        if seconds == 0.0 {
            return 0.0;
        }
        self.outputs as f64 / seconds
    }
}

/// Reads the session kind, the two arms, and the pinned window off the
/// invocation.
fn parse_request() -> Result<Request, Vec<String>> {
    let mut arguments = std::env::args().skip(1).peekable();
    // The reserved word is optional, and omitting it names the same kind,
    // since a CLI session is the only one this repository can drive.
    if arguments.peek().is_some_and(|first| first == CLI_SESSION) {
        arguments.next();
    }
    let mut positional: Vec<String> = Vec::new();
    let mut birthday = None;
    let mut runs = DEFAULT_RUNS;
    let mut seconds = DEFAULT_SECONDS;
    let mut seed = GUARD_MNEMONIC.to_string();
    let mut json = None;
    while let Some(argument) = arguments.next() {
        let mut value = |name: &str| {
            arguments
                .next()
                .ok_or_else(|| vec![format!("{name} needs a value")])
        };
        match argument.as_str() {
            "--birthday" => {
                let raw = value("--birthday")?;
                birthday = Some(
                    raw.parse()
                        .map_err(|e| vec![format!("--birthday {raw}: {e}")])?,
                );
            }
            "--runs" => {
                let raw = value("--runs")?;
                runs = raw
                    .parse()
                    .map_err(|e| vec![format!("--runs {raw}: {e}")])?;
            }
            "--seed" => seed = value("--seed")?,
            "--json" => json = Some(PathBuf::from(value("--json")?)),
            "--seconds" => {
                let raw = value("--seconds")?;
                seconds = raw
                    .parse()
                    .map_err(|e| vec![format!("--seconds {raw}: {e}")])?;
            }
            other if other.starts_with("--") => {
                return Err(vec![format!("unknown argument: {other}")]);
            }
            arm => positional.push(arm.to_string()),
        }
    }
    let [left, right] = positional.as_slice() else {
        return Err(vec![
            "sync-ab takes exactly two arms, each a commit with an optional \
             :nakednet kind; every other parameter is fixed so the comparison \
             means something"
                .to_string(),
        ]);
    };
    let specs = [Spec::parse(left), Spec::parse(right)];
    if specs[0] == specs[1] {
        return Err(vec![
            "both arms name the same commit in the same kind, which compares \
             a build against itself"
                .to_string(),
        ]);
    }
    let birthday = birthday.ok_or_else(|| {
        vec![
            "--birthday <height> is required, and every arm receives the one \
             value so they scan the same window"
                .to_string(),
        ]
    })?;
    Ok(Request {
        specs,
        birthday,
        runs,
        seconds,
        seed,
        json,
    })
}

fn compare() -> Result<(), Vec<String>> {
    let request = parse_request()?;
    let root = repo_root()?;

    let mut arms = [
        prepare(&root, &request.specs[0])?,
        prepare(&root, &request.specs[1])?,
    ];

    // A failed comparison names a session log inside an arm's checkout, so
    // the checkouts stay for reading it; the next comparison removes a
    // stale checkout before adding its own.
    rounds(&mut arms, &request)?;
    for arm in &arms {
        retire(&root, &arm.worktree);
    }
    report(&arms, request.seconds > 0);
    if let Some(path) = &request.json {
        let series: Vec<(String, Vec<Metric>)> = [BASELINE_ROLE, CANDIDATE_ROLE]
            .into_iter()
            .zip(&arms)
            .map(|(role, arm)| (benchmark(role, &arm.spec, &request), metrics(arm)))
            .collect();
        write(path, metric_format(&series))?;
    }
    Ok(())
}

// One reading of an arm as the metrics file carries it: the mean, and one
// deviation either side of it when more than one session gave it.
struct Metric {
    measure: &'static str,
    value: f64,
    bounds: Option<(f64, f64)>,
}

fn metric(measure: &'static str, samples: &[f64]) -> Metric {
    let value = mean(samples);
    Metric {
        measure,
        value,
        bounds: deviation(samples).map(|spread| (value - spread, value + spread)),
    }
}

// The arm's readings in the order the report prints them, leaving out a
// reading some session did not give.
fn metrics(arm: &Arm) -> Vec<Metric> {
    let boots: Vec<f64> = arm.boots.iter().map(Duration::as_secs_f64).collect();
    let as_f64 = |counted: Vec<u64>| counted.into_iter().map(|b| b as f64).collect::<Vec<_>>();
    [
        Some(metric(OUTPUTS_PER_SECOND, &arm.rates)),
        Some(metric(BOOT_SECONDS, &boots)),
        complete(&arm.peaks).map(|peaks| metric(PEAK_RSS_BYTES, &as_f64(peaks))),
        complete(&arm.wallet_files).map(|files| metric(WALLET_FILE_BYTES, &as_f64(files))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// The series an arm's readings join, named by the arm's role and the
// window, so a change of window starts a new series instead of bending the
// old one.
fn benchmark(role: &str, spec: &Spec, request: &Request) -> String {
    let kind = if spec.nakednet { NAKEDNET_KIND } else { "" };
    let window = if request.seconds > 0 {
        format!("{}s", request.seconds)
    } else {
        TO_COMPLETION.to_string()
    };
    let seeded = if request.seed == GUARD_MNEMONIC {
        ""
    } else {
        SEEDED
    };
    format!(
        "{role}{kind} (birthday {}, {window}{seeded})",
        request.birthday
    )
}

// The Bencher Metric Format text: a benchmark per arm, a measure per
// reading. Every name comes from a constant or a number, so nothing here
// needs escaping.
fn metric_format(series: &[(String, Vec<Metric>)]) -> String {
    let benchmarks: Vec<String> = series
        .iter()
        .map(|(benchmark, metrics)| {
            let measures: Vec<String> = metrics
                .iter()
                .map(|metric| {
                    let bounds = metric.bounds.map_or_else(String::new, |(low, high)| {
                        format!(", \"lower_value\": {low}, \"upper_value\": {high}")
                    });
                    format!(
                        "    \"{}\": {{\"value\": {}{bounds}}}",
                        metric.measure, metric.value
                    )
                })
                .collect();
            format!("  \"{benchmark}\": {{\n{}\n  }}", measures.join(",\n"))
        })
        .collect();
    format!("{{\n{}\n}}\n", benchmarks.join(",\n"))
}

// Warms both arms up, then measures them in alternating rounds.
fn rounds(arms: &mut [Arm; 2], request: &Request) -> Result<(), Vec<String>> {
    // The warm-up pays for each arm's build and is discarded: a first run
    // reads slower than its successors, and both arms deserve the same
    // treatment.
    eprintln!(
        "sync-ab: warming up both arms at birthday {}",
        request.birthday
    );
    for arm in arms.iter() {
        let _ = measure_again(arm, request, WARMUP_BUDGET)?;
    }

    for round in 1..=request.runs {
        // The order alternates, so whatever advantage a second run enjoys
        // falls on each arm equally instead of on whichever went last.
        let order: [usize; 2] = if round % 2 == 1 { [0, 1] } else { [1, 0] };
        for index in order {
            let reading = measure_again(&arms[index], request, RUN_BUDGET)?;
            eprintln!(
                "sync-ab: {round}/{} {} boot {:.1}s, {:.0} outputs/s over {} outputs, \
                 peak RSS {}, wallet file {}",
                request.runs,
                arms[index].spec.label(),
                reading.boot.as_secs_f64(),
                reading.rate(),
                reading.outputs,
                sized(reading.peak_rss, MIB, MIB_NAME),
                sized(reading.wallet_file, KIB, KIB_NAME)
            );
            let arm = &mut arms[index];
            arm.rates.push(reading.rate());
            arm.outputs.push(reading.outputs);
            arm.boots.push(reading.boot);
            arm.syncs.push(reading.sync);
            arm.peaks.push(reading.peak_rss);
            arm.wallet_files.push(reading.wallet_file);
        }
    }
    Ok(())
}

/// Gives one arm its own worktree and grafts the span into it, so neither
/// arm meets the other's build artifacts and both are measured by one
/// instrument.
fn prepare(root: &Path, spec: &Spec) -> Result<Arm, Vec<String>> {
    let worktree = root
        .parent()
        .ok_or_else(|| vec!["the repository has no parent directory".to_string()])?
        .join(format!("sync-ab-{}", spec.slug()));
    // A stale worktree from an interrupted run would refuse the add, and
    // its absence is the ordinary case rather than a failure worth showing.
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(&worktree)
        .stderr(Stdio::null())
        .status();

    eprintln!("sync-ab: preparing {}", spec.label());
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            &worktree.to_string_lossy(),
            &spec.commit,
        ],
    )?;

    let source = worktree.join(SYNC_SOURCE);
    let original = std::fs::read_to_string(&source)
        .map_err(|e| vec![format!("cannot read {}: {e}", source.display())])?;
    let instrumented = graft(&original)
        .map_err(|refusal| vec![format!("{}: {}", spec.label(), refusal.join("; "))])?;
    std::fs::write(&source, instrumented)
        .map_err(|e| vec![format!("cannot write {}: {e}", source.display())])?;

    let makefile = std::fs::read_to_string(worktree.join("Makefile.toml")).unwrap_or_default();
    let cli_source =
        std::fs::read_to_string(worktree.join("zingo-cli/src/lib.rs")).unwrap_or_default();

    Ok(Arm {
        spec: spec.clone(),
        hosted: makefile.contains("[tasks.run-cli]"),
        consents: cli_source.contains("Arg::new(\"online\")"),
        worktree,
        rates: Vec::new(),
        outputs: Vec::new(),
        boots: Vec::new(),
        syncs: Vec::new(),
        peaks: Vec::new(),
        wallet_files: Vec::new(),
    })
}

/// Wraps an arm's sync launch in the span this tool reads, leaving the call
/// inside it untouched.
// The call's shape is none of the graft's business: its argument list grew
// across the range this compares, so the text between the spawn's braces is
// carried through verbatim and only surrounded.
fn graft(source: &str) -> Result<String, Vec<String>> {
    if source.matches(SPAWN_ANCHOR).count() != 1 {
        return Err(vec![format!(
            "the sync launch anchor appears {} times, and the graft \
             instruments exactly one",
            source.matches(SPAWN_ANCHOR).count()
        )]);
    }
    let opens =
        workbench::required(source.find(SPAWN_ANCHOR), "sync launch anchor")? + SPAWN_ANCHOR.len();
    let mut depth = 1usize;
    let mut closes = None;
    for (offset, character) in source[opens..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    closes = Some(opens + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let closes =
        closes.ok_or_else(|| vec!["the sync launch never closes its block".to_string()])?;
    let launch = &source[opens..closes];
    if !launch.contains(SPAWN_CALL) {
        return Err(vec![format!(
            "the anchored block does not call {SPAWN_CALL}, so the graft \
             would instrument the wrong work"
        )]);
    }
    Ok(format!(
        "{}{}{}",
        &source[..opens],
        wrapped(launch),
        &source[closes..]
    ))
}

/// The launch surrounded by the span, as source text.
fn wrapped(launch: &str) -> String {
    format!(
        "
            let started = std::time::Instant::now();
            tracing::info!(\"{SYNC_SPAN_OPEN}\");
            let outcome = {{{launch}}};
            let elapsed = started.elapsed().as_millis();
            match &outcome {{
                Ok(result) => tracing::info!(
                    \"{SYNC_SPAN_CLOSE} {{}}{MILLIS_SUFFIX} ok {SYNC_SPAN_OUTPUTS}{{}}\",
                    elapsed,
                    u64::from(result.sapling_outputs_scanned)
                        + u64::from(result.orchard_outputs_scanned)
                        + u64::from(result.ironwood_outputs_scanned)
                ),
                Err(_) => tracing::info!(\"{SYNC_SPAN_CLOSE} {{}}{MILLIS_SUFFIX} err\", elapsed),
            }}
            outcome
        "
    )
}

/// Drives one session in the arm's worktree, reading the span out of its log.
/// - launches a second session when the first one fails
/// - prints the first failure to stderr
fn measure_again(arm: &Arm, request: &Request, budget: Duration) -> Result<Reading, Vec<String>> {
    measure(arm, request, budget).or_else(|first| {
        eprintln!(
            "sync-ab: {} session failed ({}), running it once more",
            arm.spec.label(),
            first.join("; ")
        );
        measure(arm, request, budget)
    })
}

fn measure(arm: &Arm, request: &Request, budget: Duration) -> Result<Reading, Vec<String>> {
    let scratch = arm.worktree.join("target").join("sync-ab");
    // A fresh wallet each run, so no session resumes a partial scan.
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| vec![format!("cannot make {}: {e}", scratch.display())])?;
    let log_path = scratch.join("session.log");

    // The proxy runs in its own process group, outside the one `quit` kills,
    // and exits on its own once the dead session's stdin pipe closes, so it
    // can still be running from the bundled path when the next session's
    // bundler copies over it and meets "text file busy". Unlinking the path
    // leaves the running image alone and gives the bundler a fresh
    // destination, whichever era of the bundler the arm has.
    let _ = std::fs::remove_file(
        arm.worktree
            .join("target/release")
            .join(format!("nym-proxy{}", std::env::consts::EXE_SUFFIX)),
    );

    let mut command = launcher(arm)?;
    command
        .current_dir(&arm.worktree)
        .env("RUST_LOG", LOG_FILTER);
    if arm.consents {
        command.arg("--online");
    }
    // A run to completion asks the session to shut its sync down once the
    // wallet is up to date, so the span closes on its own. A bounded run
    // keeps the sync continuous and stops it, since a session waiting for
    // its sync reads no command until then.
    if request.seconds == 0 {
        command.arg("--waitsync");
    }
    let mut child = command
        .arg("--seed")
        .arg(&request.seed)
        .arg("--birthday")
        .arg(request.birthday.to_string())
        .arg("--data-dir")
        .arg(scratch.join(WALLETS_DIR))
        .arg("--log-file")
        .arg(&log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|e| vec![format!("cannot spawn the session: {e}")])?;

    // A session that cannot consent never launches sync on its own, so it
    // is told to, the way the user does.
    if !arm.consents {
        tell(&mut child, "sync run");
    }

    let spawned = Instant::now();
    let mut launched: Option<Instant> = None;
    let mut stopped = false;
    let mut failed_since: Option<Instant> = None;
    let outcome = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if launched.is_none() && log.contains(SYNC_SPAN_OPEN) {
            launched = Some(Instant::now());
        }
        // A bounded run is stopped once its span has been open long enough.
        // The engine answers a stop with the session's own counts, so the
        // rate a partial scan reports means what a whole one's would, and a
        // range too long to finish becomes affordable to measure.
        if !stopped && request.seconds > 0 {
            if let Some(at) = launched {
                if at.elapsed() >= Duration::from_secs(request.seconds) {
                    tell(&mut child, "sync stop");
                    stopped = true;
                }
            }
        }
        if let Some((millis, outputs)) = closing_span(&log) {
            break match launched {
                Some(at) => Ok((
                    at.duration_since(spawned),
                    Duration::from_millis(millis),
                    outputs,
                )),
                None => Err(vec!["the span closed without opening".to_string()]),
            };
        }
        // A span that closes on an error carries no count, and waiting for
        // one would run the budget out on a failure already written down.
        // The CLI may still relaunch the sync on its own, so the failure
        // stands only once the grace for that has passed with no new span.
        if failed_span(&log) {
            let since = failed_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= RECOVERY_GRACE {
                break Err(vec![format!("the sync failed, see {}", log_path.display())]);
            }
        } else {
            failed_since = None;
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(vec![format!("the session exited early ({status})")]);
        }
        if spawned.elapsed() >= budget {
            break Err(vec![format!("budget of {}s exceeded", budget.as_secs())]);
        }
        std::thread::sleep(LOG_POLL_INTERVAL);
    };

    // The high-water mark is read while the session still stands at its
    // prompt, and the wallet file once the shutdown has saved it.
    let peak_rss = cli_high_water(child.id());
    session::quit(&mut child);
    let wallet_file = wallet_file_size(&scratch.join(WALLETS_DIR));
    outcome.map(|(boot, sync, outputs)| Reading {
        boot,
        sync,
        outputs,
        peak_rss,
        wallet_file,
    })
}

/// - writes to the child's stdin
fn tell(child: &mut Child, line: &str) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "{line}");
        let _ = stdin.flush();
    }
}

/// The command that builds and launches one arm's CLI.
// An arm carrying the run-cli task is launched through it, so the mixnet
// build and the bundled proxy are exactly what that task provisions. An arm
// predating the task is built and run directly, which is the only way to
// reach the era this comparison looks back into.
fn launcher(arm: &Arm) -> Result<Command, Vec<String>> {
    if arm.hosted {
        let mut command = cargo_make("run-cli");
        command.arg("--release");
        if arm.spec.nakednet {
            command.arg("--nakednet");
        }
        return Ok(command);
    }
    let mut build = Command::new("cargo");
    build
        .current_dir(&arm.worktree)
        .args(["build", "--release", "-p", "zingo-cli"]);
    if arm.spec.nakednet {
        build.arg("--no-default-features");
    }
    let built = build
        .status()
        .map_err(|e| vec![format!("cannot build {}: {e}", arm.spec.label())])?;
    if !built.success() {
        return Err(vec![format!(
            "{} does not build its CLI ({built})",
            arm.spec.label()
        )]);
    }
    Ok(Command::new(arm.worktree.join("target/release/zingo-cli")))
}

// A close on `err` is a failure only while no later span has opened: the
// CLI's own recovery relaunches a failed sync, and that span's close is the
// one that counts.
fn failed_span(log: &str) -> bool {
    log.rsplit_once(SYNC_SPAN_CLOSE).is_some_and(|(_, tail)| {
        tail.split_whitespace().nth(1) == Some(SPAN_FAILED) && !tail.contains(SYNC_SPAN_OPEN)
    })
}

/// The engine's own duration and output count, from the closing line.
fn closing_span(log: &str) -> Option<(u64, u64)> {
    log.split(SYNC_SPAN_CLOSE).skip(1).find_map(closed_segment)
}

// The library logs a close of its own before the grafted one, without a
// count, so the reading comes from the first segment that carries one.
fn closed_segment(tail: &str) -> Option<(u64, u64)> {
    let digits: String = tail
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    // A truncated final line can hold the marker before its whole duration,
    // so a reading is taken only once the unit follows the digits.
    if !tail
        .trim_start()
        .get(digits.len()..)?
        .starts_with(MILLIS_SUFFIX)
    {
        return None;
    }
    // A failed sync closes its span without a count, and a rate over work
    // that never finished would be a fiction.
    let counted: String = tail
        .split(SYNC_SPAN_OUTPUTS)
        .nth(1)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some((digits.parse().ok()?, counted.parse().ok()?))
}

// The CLI is the launched process itself for a direct launch and a
// descendant of it for a hosted one, so the search walks down from the
// launched process through the kernel's per-thread children lists and
// stops at the first process named for the CLI, which leaves the proxy
// below it unread and touches a handful of entries rather than every
// process on the host.
/// - reads the stat, the children lists, and the CLI's status under `/proc`
fn cli_high_water(launched: u32) -> Option<u64> {
    let mut pending = vec![launched];
    while let Some(pid) = pending.pop() {
        let entry = Path::new(PROC).join(pid.to_string());
        if std::fs::read_to_string(entry.join(STAT_FILE)).is_ok_and(|stat| names_cli(&stat)) {
            return high_water(&std::fs::read_to_string(entry.join(STATUS_FILE)).ok()?);
        }
        pending.extend(children_of(&entry));
    }
    None
}

// The processes every thread of the process at `entry` spawned, since a
// child is listed under the thread that spawned it.
fn children_of(entry: &Path) -> Vec<u32> {
    std::fs::read_dir(entry.join(TASK_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join(CHILDREN_FILE)).ok())
        .flat_map(|listed| {
            listed
                .split_whitespace()
                .filter_map(|child| child.parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

// The command name sits in parentheses and may hold spaces or parentheses
// of its own, so it is read up to the last closing parenthesis.
fn names_cli(stat: &str) -> bool {
    stat.split_once('(')
        .and_then(|(_, named)| named.rsplit_once(')'))
        .is_some_and(|(name, _)| name == CLI_COMM)
}

// The high-water mark a process's status reports, in bytes.
fn high_water(status: &str) -> Option<u64> {
    let kibibytes: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix(HIGH_WATER_FIELD))?
        .trim()
        .strip_suffix(HIGH_WATER_UNIT)?
        .trim()
        .parse()
        .ok()?;
    Some(kibibytes * KIB)
}

/// - reads the wallet file's metadata under `wallets`
fn wallet_file_size(wallets: &Path) -> Option<u64> {
    std::fs::metadata(wallets.join(WALLET_FILE))
        .ok()
        .map(|metadata| metadata.len())
}

/// Prints both arms, their spread, and whether the window held.
fn report(arms: &[Arm; 2], bounded: bool) {
    println!(
        "\nSYNC_AB_TAG: {} vs {}",
        arms[0].spec.label(),
        arms[1].spec.label()
    );
    for arm in arms {
        let rate = mean(&arm.rates);
        print!("{:>22}  mean {rate:>7.0} outputs/s", arm.spec.label());
        if let Some(spread) = deviation(&arm.rates) {
            print!("  stdev {spread:>6.0}");
        }
        println!(
            "  samples {:?}",
            arm.rates
                .iter()
                .map(|rate| format!("{rate:.0}"))
                .collect::<Vec<_>>()
        );
        let boots: Vec<f64> = arm.boots.iter().map(Duration::as_secs_f64).collect();
        println!(
            "{:>22}  boot {:.1}s mean (build and bootstrap, wall clock)",
            "",
            mean(&boots)
        );
        println!(
            "{:>22}  peak RSS {} mean  wallet file {} mean",
            "",
            sized(mean_bytes(&arm.peaks), MIB, MIB_NAME),
            sized(mean_bytes(&arm.wallet_files), KIB, KIB_NAME)
        );
    }

    // A run that finishes scans a fixed window, so its output counts
    // should agree and a disagreement means the arms did different work. A
    // bounded run scans whatever its seconds reach, so its counts differ by
    // design and the fixed quantity is the duration instead. Checking the
    // counts there would fire on the tool's own design.
    let (measured, (low, high), tolerance) = if bounded {
        let spans: Vec<u64> = arms
            .iter()
            .flat_map(|arm| arm.syncs.iter().map(|span| span.as_millis() as u64))
            .collect();
        ("milliseconds scanned", span(&spans), SPAN_DRIFT_TOLERANCE)
    } else {
        let counted: Vec<u64> = arms.iter().flat_map(|arm| arm.outputs.clone()).collect();
        ("outputs scanned", span(&counted), OUTPUT_DRIFT_TOLERANCE)
    };
    let drift = change(low as f64, high as f64);
    let cause = if bounded {
        "a stop landing after the batch in flight"
    } else {
        "the tip advancing mid-run"
    };
    println!("{measured}: {low} to {high} ({drift:.2}% drift, {cause})");
    if drift > tolerance {
        let what = if bounded {
            "scanned for materially different spans"
        } else {
            "scanned materially different work"
        };
        println!("  WARNING: the arms {what}, so the rates are not comparable");
    }

    let (left, right) = (mean(&arms[0].rates), mean(&arms[1].rates));
    let gap = change(left, right);
    let noise = deviation(&arms[0].rates)
        .into_iter()
        .chain(deviation(&arms[1].rates))
        .fold(0.0, f64::max);
    println!(
        "difference: {gap:+.1}% ({} against {})",
        arms[1].spec.label(),
        arms[0].spec.label()
    );
    if (right - left).abs() < noise {
        println!("  within the noisier arm's own deviation, so indistinguishable");
    }
    for (what, left, right) in [
        (
            "peak RSS",
            mean_bytes(&arms[0].peaks),
            mean_bytes(&arms[1].peaks),
        ),
        (
            "wallet file",
            mean_bytes(&arms[0].wallet_files),
            mean_bytes(&arms[1].wallet_files),
        ),
    ] {
        match (left, right) {
            (Some(left), Some(right)) => {
                println!("{what}: {:+.1}%", change(left as f64, right as f64));
            }
            _ => println!("{what}: {UNAVAILABLE}, a session gave no reading"),
        }
    }
    if bounded {
        println!("  the memory and the file follow the span scanned as well as the arm");
    }
}

// The change from `left` to `right` as a percentage of `left`.
fn change(left: f64, right: f64) -> f64 {
    if left == 0.0 {
        return 0.0;
    }
    (right - left) / left * PERCENT
}

// The arithmetic mean of sampled byte counts, undefined when there are none
// or when a session gave no reading, so the arms' means cover the same
// sessions as their rates.
fn mean_bytes(samples: &[Option<u64>]) -> Option<u64> {
    let counted = complete(samples)?;
    Some(counted.iter().sum::<u64>() / counted.len() as u64)
}

// The sampled byte counts when every session gave one, and nothing when
// any session did not.
fn complete(samples: &[Option<u64>]) -> Option<Vec<u64>> {
    let counted: Option<Vec<u64>> = samples.iter().copied().collect();
    counted.filter(|counted| !counted.is_empty())
}

// A byte count in `unit`, or the word for a reading the platform did not
// give.
fn sized(bytes: Option<u64>, unit: u64, name: &str) -> String {
    bytes.map_or_else(
        || UNAVAILABLE.to_string(),
        |bytes| format!("{:.0} {name}", bytes as f64 / unit as f64),
    )
}

/// The lowest and highest of a sampled series.
fn span(samples: &[u64]) -> (u64, u64) {
    (
        samples.iter().copied().min().unwrap_or_default(),
        samples.iter().copied().max().unwrap_or_default(),
    )
}

/// The arithmetic mean of the sampled rates.
fn mean(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

/// The sample standard deviation, which one sample leaves undefined.
fn deviation(samples: &[f64]) -> Option<f64> {
    let freedom = samples.len().checked_sub(BESSEL_CORRECTION)?;
    if freedom == 0 {
        return None;
    }
    let centre = mean(samples);
    let squares: f64 = samples.iter().map(|s| (s - centre).powi(2)).sum();
    Some((squares / freedom as f64).sqrt())
}

/// Removes an arm's worktree, so a comparison leaves no checkout behind.
fn retire(root: &Path, worktree: &Path) {
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(worktree)
        .status();
}

/// Runs one git command, refusing with its stderr.
fn git(root: &Path, args: &[&str]) -> Result<(), Vec<String>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| vec![format!("cannot run git {}: {e}", args.join(" "))])?;
    if output.status.success() {
        return Ok(());
    }
    Err(vec![format!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(seconds: u64, seed: &str) -> Request {
        Request {
            specs: [Spec::parse("a"), Spec::parse("b")],
            birthday: 2_571_710,
            runs: DEFAULT_RUNS,
            seconds,
            seed: seed.to_string(),
            json: None,
        }
    }

    #[test]
    fn benchmark_names_the_role_the_kind_and_the_window() {
        let bounded = request(90, GUARD_MNEMONIC);
        assert_eq!(
            benchmark(BASELINE_ROLE, &Spec::parse("a:nakednet"), &bounded),
            "baseline:nakednet (birthday 2571710, 90s)"
        );
        let whole = request(0, "some other mnemonic");
        assert_eq!(
            benchmark(CANDIDATE_ROLE, &Spec::parse("b"), &whole),
            "candidate (birthday 2571710, to completion, seeded)"
        );
    }

    #[test]
    fn metric_format_bounds_a_reading_only_where_a_deviation_exists() {
        let series = vec![(
            "candidate (birthday 1, 90s)".to_string(),
            vec![
                metric(OUTPUTS_PER_SECOND, &[100.0, 110.0, 120.0]),
                metric(BOOT_SECONDS, &[2.5]),
            ],
        )];
        assert_eq!(
            metric_format(&series),
            "{\n  \"candidate (birthday 1, 90s)\": {\n    \"outputs-per-second\": \
             {\"value\": 110, \"lower_value\": 100, \"upper_value\": 120},\n    \
             \"boot-seconds\": {\"value\": 2.5}\n  }\n}\n"
        );
    }

    /// The launch as the beta tag writes it, five arguments on one line.
    const EARLY_ERA: &str = "        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(client, &chain_type, wallet, sync_mode, sync_config).await
        });
        self.sync_handle = Some(sync_handle);
";

    /// The launch as dev writes it, six arguments across several lines.
    const LATER_ERA: &str = "        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(
                client,
                &chain_type,
                wallet,
                sync_mode,
                progress_sender,
                sync_config,
            )
            .await
        });
        self.sync_handle = Some(sync_handle);
";

    /// HYPOTHESIS: the graft carries every era's call through untouched and
    /// surrounds it with the span. Falsified if a call is altered or a
    /// marker is missing.
    #[test]
    fn the_graft_surrounds_each_era_without_touching_its_call() -> Result<(), Vec<String>> {
        for era in [EARLY_ERA, LATER_ERA] {
            let grafted = graft(era)?;
            assert!(
                grafted.contains(SYNC_SPAN_OPEN) && grafted.contains(SYNC_SPAN_CLOSE),
                "the span must open and close: {grafted}"
            );
            assert!(
                grafted.contains(SYNC_SPAN_OUTPUTS),
                "the close must carry the output count: {grafted}"
            );
            let after_anchor = workbench::required(
                era.split_once(SPAWN_ANCHOR),
                "sync launch anchor in the era",
            )?
            .1;
            let call = workbench::required(
                after_anchor.rsplit_once("});"),
                "closing of the launch in the era",
            )?
            .0;
            assert!(
                grafted.contains(call.trim_end()),
                "the call must survive verbatim: {grafted}"
            );
            assert!(
                grafted.ends_with("        self.sync_handle = Some(sync_handle);\n"),
                "everything after the launch must survive: {grafted}"
            );
        }
        Ok(())
    }

    /// HYPOTHESIS: a graft that cannot find exactly one launch refuses
    /// rather than instrumenting the wrong work. Falsified if either shape
    /// returns a grafted source.
    #[test]
    fn an_unanchorable_source_is_refused() {
        assert!(graft("fn main() {}").is_err(), "no anchor must refuse");
        let doubled = format!("{EARLY_ERA}{LATER_ERA}");
        assert!(graft(&doubled).is_err(), "two anchors must refuse");
    }

    /// HYPOTHESIS: an anchored block that scans nothing is refused, since
    /// instrumenting it would time work this tool does not mean.
    #[test]
    fn an_anchored_block_that_does_not_scan_is_refused() {
        let impostor = "        let sync_handle = tokio::spawn(async move {
            some_other_task().await
        });
";
        assert!(graft(impostor).is_err(), "a foreign block must refuse");
    }

    /// HYPOTHESIS: an arm's kind is read off its suffix, and the label
    /// keeps it. Falsified if a plain commit reads as nakednet or the
    /// suffix survives into the commit.
    #[test]
    fn an_arm_reads_its_build_kind_off_its_suffix() {
        let plain = Spec::parse("dev");
        assert_eq!(plain.commit, "dev");
        assert!(!plain.nakednet);
        let bare = Spec::parse("dev:nakednet");
        assert_eq!(bare.commit, "dev");
        assert!(bare.nakednet);
        assert_eq!(bare.label(), "dev:nakednet");
    }

    /// HYPOTHESIS: a closing line is read only once its duration and count
    /// are both whole. Falsified if a truncated line yields a reading.
    #[test]
    fn a_truncated_closing_line_yields_no_reading() {
        assert_eq!(
            closing_span("SYNC_SPAN=close 1250ms ok outputs=8192\n"),
            Some((1250, 8192))
        );
        assert_eq!(closing_span("SYNC_SPAN=close 12"), None);
        assert_eq!(closing_span("SYNC_SPAN=close 1250ms err\n"), None);
    }

    // HYPOTHESIS: a span that closed on an error is told apart from one
    // that finished, one still open, one whose close is cut short, and one
    // the CLI relaunched after the error.
    #[test]
    fn a_failed_close_is_read_as_a_failure() {
        assert!(failed_span("INFO sync: SYNC_SPAN=close 137361ms err\n"));
        assert!(!failed_span(
            "INFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n"
        ));
        assert!(!failed_span("INFO sync: SYNC_SPAN=open\n"));
        assert!(!failed_span("INFO sync: SYNC_SPAN=close 1250ms er"));
        assert!(!failed_span(
            "INFO sync: SYNC_SPAN=close 1200ms err\nINFO sync: SYNC_SPAN=open\n"
        ));
        assert!(!failed_span(
            "INFO sync: SYNC_SPAN=close 1200ms err\nINFO sync: SYNC_SPAN=open\n\
             INFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n"
        ));
    }

    #[test]
    fn a_bare_close_before_the_counted_one_is_skipped() {
        let log = "INFO sync: SYNC_SPAN=close 1250ms ok\nINFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n";
        assert_eq!(closing_span(log), Some((1250, 8192)));
    }

    // HYPOTHESIS: a stat line is matched on the command name alone, read up
    // to its last closing parenthesis. Falsified if the proxy, the builder,
    // or a name holding a parenthesis is taken for the CLI.
    #[test]
    fn a_stat_line_names_the_cli() {
        assert!(names_cli("4242 (zingo-cli) S 4200 4100 4100 0 -1"));
        assert!(!names_cli("4243 (nym-proxy) S 4242 4243 4100 0 -1"));
        assert!(!names_cli("4244 (cargo) S 4200 4100 4100 0 -1"));
        assert!(!names_cli("4245 (zingo-cli (old)) S 4200 4100 4100 0 -1"));
        assert!(!names_cli("no fields at all"));
    }

    // HYPOTHESIS: a byte mean is undefined as soon as one session gave no
    // reading, so the arms are never averaged over different sessions.
    #[test]
    fn a_missing_reading_leaves_the_mean_undefined() {
        assert_eq!(mean_bytes(&[Some(KIB), Some(3 * KIB)]), Some(2 * KIB));
        assert_eq!(mean_bytes(&[Some(KIB), None]), None);
        assert_eq!(mean_bytes(&[]), None);
    }

    // HYPOTHESIS: the high-water mark is read in bytes off its own status
    // line. Falsified if another memory line is taken for it or a status
    // without one yields a reading.
    #[test]
    fn the_high_water_mark_is_read_in_bytes() {
        let status =
            "Name:\tzingo-cli\nVmPeak:\t    2048 kB\nVmHWM:\t    1536 kB\nVmRSS:\t    1024 kB\n";
        assert_eq!(high_water(status), Some(1536 * KIB));
        assert_eq!(high_water("Name:\tzingo-cli\nVmRSS:\t 1024 kB\n"), None);
    }
}
