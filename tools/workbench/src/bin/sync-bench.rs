//! Times sync inside a real `run-cli --online` session, mixnet boot included.
//!
//! # Running it
//!
//! Three scenarios cover what this tool measures. Each one requires
//! `--birthday <height>`, takes `--runs <n>` for the sessions per arm, and
//! stamps its report with `--label <text>`.
//!
//! The mixnet arm alone, which is the default because the mixnet is a
//! default capability (ADR 0026):
//!
//! ```text
//! makers sync-bench --birthday 3500000 --runs 3 --label baseline
//! ```
//!
//! The clearnet arm alone, which is the same session with the mixnet left
//! out of the build:
//!
//! ```text
//! makers sync-bench --clearnet --birthday 3500000 --runs 3 --label baseline
//! ```
//!
//! Both arms alternated round by round, which is how a difference is
//! attributed to the mixnet rather than to the hour the run fell in:
//!
//! ```text
//! makers sync-bench --compare --birthday 3500000 --runs 3 --label baseline
//! ```
//!
//! Three constraints catch a first run. The birthday is required, and both
//! arms of a comparison scan from the one value, so name a height near the
//! tip; a session that has not synced within fifteen minutes counts as
//! failed. The `--clearnet` and `--compare` flags contradict each other,
//! and the tool refuses the pair rather than guessing which one was meant.
//! Each arm builds into its own directory under `target/sync-bench/builds`,
//! so alternating the arms costs no rebuild, at the price of two release
//! builds on disk.
//!
//! # What it measures
//!
//! An in-process harness measures the sync engine alone, which answers
//! nothing about a session where four proven exits bootstrap beside the
//! scan. This drives the CLI the way a user does: `makers run-cli --online`
//! spawns the proxy, proves the quartet, runs the Server-Selection Sweep,
//! and only then launches sync.
//!
//! The headline number comes from the sync engine's own clock. The task
//! that runs the scan opens and closes the Sync Span around it and logs the
//! elapsed milliseconds, so this reads one measurement taken inside the
//! task that did the work. Nothing here polls a status or watches a prompt
//! redraw, and no cross-process timestamp arithmetic enters the number.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// Agreeing representations of `zingolib::lightclient::sync`'s minted log
/// markers, restated because the workbench is deliberately std-only and
/// takes no production dependency; these must not drift from that crate.
const SYNC_SPAN_OPEN: &str = "SYNC_SPAN=open";

/// The marker closing the Sync Span, followed by the engine's own duration.
const SYNC_SPAN_CLOSE: &str = "SYNC_SPAN=close";

/// The unit suffix the closing marker's duration carries.
const MILLIS_SUFFIX: &str = "ms";

/// The log filter the session runs under, since an empty one silences the
/// markers this reads.
const LOG_FILTER: &str = "info";

/// The indexer every run pins, so arms compare like with like.
const PINNED_INDEXER: &str = "https://zec.rocks:443";

/// The zingo-cli feature a clearnet arm needs to go online at all, since a
/// build without the mixnet otherwise refuses every online consent act.
const CLEARNET_ONLINE_FEATURE: &str = "clearnet-test-mode";

/// The processes whose CPU is attributed across a session's windows.
///
/// Sync never travels the mixnet — `LightClient` holds a plain
/// `GrpcIndexer` and ADR 0027 keeps clearnet serving sync — so a mixnet
/// session cannot slow the scan by routing it. What it can do is compete
/// for cores, and these are the two competitors: the wallet doing the scan
/// and the proxy doing Sphinx crypto and cover traffic.
const WALLET_PROCESS: &str = "zingo-cli";

/// The proxy child, absent entirely from a clearnet arm.
const PROXY_PROCESS: &str = "nym-proxy";

/// Kernel clock ticks per second, the unit `/proc/<pid>/stat` counts CPU in.
const CLOCK_TICKS_PER_SECOND: f64 = 100.0;

/// One live process, as `/proc/<pid>/stat` reports it.
struct ProcessStat {
    pid: u32,
    parent: u32,
    comm: String,
    /// Core-seconds this process has burned over its whole life.
    cpu: f64,
}

/// One `/proc/<pid>/stat` line's pid, parent, name, and core-seconds.
fn parse_stat(text: &str) -> Option<ProcessStat> {
    // The comm field is parenthesised and may itself contain spaces, so the
    // fields after it are found from the closing parenthesis rather than by
    // splitting the whole line.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let after: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // State is the first field after comm and ppid the second, while utime
    // and stime are fields 14 and 15 of stat, the 12th and 13th after comm.
    let utime: f64 = after.get(11)?.parse().ok()?;
    let stime: f64 = after.get(12)?.parse().ok()?;
    Some(ProcessStat {
        pid: text[..open].trim().parse().ok()?,
        parent: after.get(1)?.parse().ok()?,
        comm: text[open + 1..close].to_string(),
        cpu: (utime + stime) / CLOCK_TICKS_PER_SECOND,
    })
}

/// Every live process the kernel will show us.
fn process_table() -> Vec<ProcessStat> {
    let mut table = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return table;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(stat) = parse_stat(&text) {
            table.push(stat);
        }
    }
    table
}

/// The pids descending from `root`, `root` itself included.
fn descendants(root: u32, table: &[ProcessStat]) -> HashSet<u32> {
    let mut family = HashSet::from([root]);
    let mut grew = true;
    while grew {
        grew = false;
        for stat in table {
            if family.contains(&stat.parent) && family.insert(stat.pid) {
                grew = true;
            }
        }
    }
    family
}

/// What one process had burned when first seen and when last seen.
struct Reading {
    baseline: f64,
    latest: f64,
}

/// Core-seconds one role's processes burned inside a window, held as a
/// reading per pid so a process that exits keeps what it burned instead of
/// subtracting it, and one that starts is counted from its own birth.
struct Ledger {
    readings: HashMap<u32, Reading>,
}

impl Ledger {
    fn new() -> Self {
        Ledger {
            readings: HashMap::new(),
        }
    }

    /// Records what `pid` had burned at this sample.
    fn observe(&mut self, pid: u32, cpu: f64) {
        self.readings
            .entry(pid)
            .or_insert(Reading {
                baseline: cpu,
                latest: cpu,
            })
            .latest = cpu;
    }

    /// Core-seconds every recorded process burned since it was first seen.
    fn total(&self) -> f64 {
        self.readings
            .values()
            .map(|reading| reading.latest - reading.baseline)
            .sum()
    }
}

/// Core-seconds one window charged to each role.
struct Cpu {
    wallet: f64,
    proxy: f64,
}

/// The ledgers of one window — the Boot Window or the Scan Window — which
/// observes only while it is open, so a process alive at the boundary ends
/// the Boot Window at the very reading that opens the Scan Window.
struct Window {
    wallet: Ledger,
    proxy: Ledger,
}

impl Window {
    fn new() -> Self {
        Window {
            wallet: Ledger::new(),
            proxy: Ledger::new(),
        }
    }

    /// Records every process of `family` this window attributes CPU to.
    fn observe(&mut self, table: &[ProcessStat], family: &HashSet<u32>) {
        for stat in table.iter().filter(|stat| family.contains(&stat.pid)) {
            match stat.comm.as_str() {
                WALLET_PROCESS => self.wallet.observe(stat.pid, stat.cpu),
                PROXY_PROCESS => self.proxy.observe(stat.pid, stat.cpu),
                _ => {}
            }
        }
    }

    /// What each role burned over this window.
    fn totals(&self) -> Cpu {
        Cpu {
            wallet: self.wallet.total(),
            proxy: self.proxy.total(),
        }
    }
}

/// A BIP-39 mnemonic holding no funds, so the scan measures pure scanning.
const GUARD_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// The directory under `target` the bench owns.
const BENCH_DIR: &str = "sync-bench";

/// Sessions one arm runs when `--runs` names no other count.
const DEFAULT_RUNS: usize = 3;

/// How long one session may take before it counts as failed.
const RUN_BUDGET: Duration = Duration::from_secs(900);

/// How often the driver rereads the log for the markers.
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The degree of freedom a sample standard deviation gives up to its mean.
const BESSEL_CORRECTION: usize = 1;

fn main() {
    run("sync-bench", bench, |()| {})
}

/// One session's outcome.
enum Outcome {
    /// Boot measured by wall clock, sync by the engine's own.
    Synced {
        boot: Duration,
        sync: Duration,
        /// Core-seconds each role burned over the Boot Window, which is
        /// where a mixnet session spawns its proxy and proves its quartet.
        boot_cpu: Cpu,
        /// Core-seconds each role burned over the Scan Window.
        scan_cpu: Cpu,
    },
    Failed {
        detail: String,
    },
}

/// One arm of a comparison, named by the transport its build carries.
#[derive(Clone, Copy)]
enum Arm {
    /// The mixnet is a default capability (ADR 0026), so this session boots
    /// a proxy, proves a quartet, and sweeps before the scan.
    Mixnet,
    /// A build without the mixnet, which is what attributes the mixnet's
    /// cost to the mixnet rather than leaving it inside one number.
    Clearnet,
}

impl Arm {
    /// The word this arm is reported and its build directory named by.
    fn name(self) -> &'static str {
        match self {
            Arm::Mixnet => "mixnet",
            Arm::Clearnet => "clearnet",
        }
    }

    /// Whether this arm's build leaves the mixnet out.
    fn is_clearnet(self) -> bool {
        matches!(self, Arm::Clearnet)
    }
}

/// One arm's measurements over all its sessions.
struct Samples {
    boots: Vec<Duration>,
    syncs: Vec<Duration>,
    boot_cpus: Vec<Cpu>,
    scan_cpus: Vec<Cpu>,
}

impl Samples {
    fn new() -> Self {
        Samples {
            boots: Vec::new(),
            syncs: Vec::new(),
            boot_cpus: Vec::new(),
            scan_cpus: Vec::new(),
        }
    }
}

/// What the invocation asked for.
struct Request {
    birthday: u32,
    label: String,
    runs: usize,
    /// The arms to run, alternated round by round when there is more than
    /// one so machine load falls on both equally.
    arms: Vec<Arm>,
}

fn parse_request() -> Result<Request, Vec<String>> {
    let mut args = std::env::args().skip(1);
    let mut birthday = None;
    let mut label = String::from("unlabelled");
    let mut runs = DEFAULT_RUNS;
    let mut clearnet = false;
    let mut compare = false;
    while let Some(argument) = args.next() {
        let mut value = |name: &str| {
            args.next()
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
            "--clearnet" => clearnet = true,
            "--compare" => compare = true,
            "--label" => label = value("--label")?,
            "--runs" => {
                let raw = value("--runs")?;
                runs = raw
                    .parse()
                    .map_err(|e| vec![format!("--runs {raw}: {e}")])?;
            }
            other => return Err(vec![format!("unknown argument: {other}")]),
        }
    }
    if clearnet && compare {
        return Err(vec![
            "--clearnet and --compare contradict each other; --clearnet runs \
             the clearnet arm alone, --compare alternates both"
                .to_string(),
        ]);
    }
    let birthday = birthday.ok_or_else(|| {
        vec![
            "--birthday <height> is required, and both arms of an A/B must use \
             the same one so they scan the same window"
                .to_string(),
        ]
    })?;
    let arms = match (compare, clearnet) {
        (true, _) => vec![Arm::Mixnet, Arm::Clearnet],
        (false, true) => vec![Arm::Clearnet],
        (false, false) => vec![Arm::Mixnet],
    };
    Ok(Request {
        birthday,
        label,
        runs,
        arms,
    })
}

fn bench() -> Result<(), Vec<String>> {
    let request = parse_request()?;
    let root = repo_root()?;

    for arm in &request.arms {
        prebuild(&root, *arm)?;
    }

    let mut samples: Vec<Samples> = request.arms.iter().map(|_| Samples::new()).collect();
    // Round by round rather than arm by arm, so a machine that is busier in
    // the second half of the hour charges both arms for it.
    for round in 0..request.runs {
        for (index, arm) in request.arms.iter().enumerate() {
            match session(&root, request.birthday, *arm)? {
                Outcome::Synced {
                    boot,
                    sync,
                    boot_cpu,
                    scan_cpu,
                } => {
                    eprintln!(
                        "sync-bench: {:2}/{} {} boot {:.1}s (wallet {:.1}, proxy {:.1} core-s), \
                         sync {:.3}s (wallet {:.1}, proxy {:.1} core-s)",
                        round + 1,
                        request.runs,
                        arm.name(),
                        boot.as_secs_f64(),
                        boot_cpu.wallet,
                        boot_cpu.proxy,
                        sync.as_secs_f64(),
                        scan_cpu.wallet,
                        scan_cpu.proxy
                    );
                    samples[index].boots.push(boot);
                    samples[index].syncs.push(sync);
                    samples[index].boot_cpus.push(boot_cpu);
                    samples[index].scan_cpus.push(scan_cpu);
                }
                Outcome::Failed { detail } => eprintln!(
                    "sync-bench: {:2}/{} {} failed: {detail}",
                    round + 1,
                    request.runs,
                    arm.name()
                ),
            }
        }
    }
    for (index, arm) in request.arms.iter().enumerate() {
        report(
            &format!("{} {}", request.label, arm.name()),
            &samples[index],
        );
    }
    Ok(())
}

/// Where the bench keeps its builds and its throwaway wallets.
fn bench_dir(root: &Path) -> PathBuf {
    root.join("target").join(BENCH_DIR)
}

/// The directory one arm builds into, held apart from the other arm's so
/// alternating between two feature sets costs no rebuild at each switch.
fn build_dir(root: &Path, arm: Arm) -> PathBuf {
    bench_dir(root).join("builds").join(arm.name())
}

/// The `makers run-cli` invocation the prebuild and every session share.
///
/// Both go through here so the two cannot drift in profile, features, or
/// build directory, and the profile is release because an unoptimized
/// core-second answers nothing about what the mixnet costs a user.
fn run_cli(root: &Path, arm: Arm) -> Command {
    let mut command = Command::new("makers");
    command
        .current_dir(root)
        .env("RUST_LOG", LOG_FILTER)
        .arg("run-cli")
        .arg("--release")
        .arg("--target-dir")
        .arg(build_dir(root, arm));
    if arm.is_clearnet() {
        // Naming the feature here rather than letting `run-cli` assume it:
        // a build without the mixnet refuses every online consent act (ADR
        // 0024), and `clearnet-test-mode` is what suspends that refusal.
        // Suspending a ratified refusal is a deliberate act, so the tool
        // that wants it says so.
        command
            .arg("--clearnet")
            .arg("--features")
            .arg(CLEARNET_ONLINE_FEATURE);
    }
    command
}

/// Builds the CLI once before the runs, so no session spends its budget
/// compiling and no Boot Window carries a release build inside it.
fn prebuild(root: &Path, arm: Arm) -> Result<(), Vec<String>> {
    eprintln!("sync-bench: building the {} arm", arm.name());
    let status = run_cli(root, arm)
        .arg("--build-only")
        .status()
        .map_err(|e| vec![format!("cannot spawn makers run-cli --build-only: {e}")])?;
    if status.success() {
        return Ok(());
    }
    Err(vec![format!("the prebuild failed ({status})")])
}

/// Drives one session, reading both markers out of its log.
fn session(root: &Path, birthday: u32, arm: Arm) -> Result<Outcome, Vec<String>> {
    // Beside the builds rather than over them, since wiping this must not
    // cost the arms the compile the prebuild already paid for.
    let scratch = bench_dir(root).join("session");
    // A fresh wallet each run, so no session resumes a partial scan.
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| vec![format!("cannot make {}: {e}", scratch.display())])?;
    let log_path = scratch.join("session.log");

    let mut command = run_cli(root, arm);
    command
        .arg("--online")
        .arg("--server")
        .arg(PINNED_INDEXER)
        .arg("--seed")
        .arg(GUARD_MNEMONIC)
        .arg("--birthday")
        .arg(birthday.to_string())
        .arg("--data-dir")
        .arg(scratch.join("wallets"))
        .arg("--log-file")
        .arg(&log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|e| vec![format!("cannot spawn makers run-cli: {e}")])?;

    let spawned = Instant::now();
    let mut launched: Option<Instant> = None;
    let mut boot_window = Window::new();
    let mut scan_window = Window::new();
    let outcome = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        let opening = launched.is_none() && log.contains(SYNC_SPAN_OPEN);
        if opening {
            launched = Some(Instant::now());
        }
        // Sampled every poll from the spawn rather than at the Sync Span's
        // two edges, so the Boot Window is measured and a process that exits
        // before the close still contributes what it burned.
        let table = process_table();
        let family = descendants(child.id(), &table);
        if launched.is_none() || opening {
            boot_window.observe(&table, &family);
        }
        if launched.is_some() {
            scan_window.observe(&table, &family);
        }
        if let Some(millis) = closing_millis(&log) {
            break match launched {
                Some(at) => Outcome::Synced {
                    boot: at.duration_since(spawned),
                    sync: Duration::from_millis(millis),
                    boot_cpu: boot_window.totals(),
                    scan_cpu: scan_window.totals(),
                },
                None => Outcome::Failed {
                    detail: "the Sync Span closed without opening".to_string(),
                },
            };
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Outcome::Failed {
                detail: format!("the session exited early ({status})"),
            };
        }
        if spawned.elapsed() >= RUN_BUDGET {
            break Outcome::Failed {
                detail: format!("budget of {}s exceeded", RUN_BUDGET.as_secs()),
            };
        }
        std::thread::sleep(LOG_POLL_INTERVAL);
    };

    quit(&mut child);
    Ok(outcome)
}

/// The engine's own duration, from the line closing the Sync Span.
fn closing_millis(log: &str) -> Option<u64> {
    let tail = log.split(SYNC_SPAN_CLOSE).nth(1)?;
    let digits: String = tail
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    // A truncated final line can hold the marker before its whole duration,
    // so a reading is taken only once the unit follows the digits.
    tail.trim_start()
        .get(digits.len()..)?
        .starts_with(MILLIS_SUFFIX)
        .then(|| digits.parse().ok())
        .flatten()
}

/// Ends the session the way a user does, then reaps it.
fn quit(child: &mut Child) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "quit");
        let _ = stdin.flush();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Prints every measurement against the label the arm carries.
fn report(label: &str, samples: &Samples) {
    let boot = &samples.boot_cpus;
    let scan = &samples.scan_cpus;
    println!("\nSYNC_BENCH_TAG: {label}");
    println!("sessions: {}", samples.syncs.len());
    distribution("Boot Window (wall clock)", &seconds(&samples.boots), "s");
    distribution("Sync Span (engine clock)", &seconds(&samples.syncs), "s");
    distribution(
        "wallet CPU over the Boot Window",
        &column(boot, |cpu| cpu.wallet),
        "core-s",
    );
    distribution(
        "proxy CPU over the Boot Window",
        &column(boot, |cpu| cpu.proxy),
        "core-s",
    );
    distribution(
        "wallet CPU over the Scan Window",
        &column(scan, |cpu| cpu.wallet),
        "core-s",
    );
    distribution(
        "proxy CPU over the Scan Window",
        &column(scan, |cpu| cpu.proxy),
        "core-s",
    );
}

/// One role's core-seconds out of every session's pair.
fn column(samples: &[Cpu], role: impl Fn(&Cpu) -> f64) -> Vec<f64> {
    samples.iter().map(role).collect()
}

/// The same durations as a count of seconds apiece.
fn seconds(samples: &[Duration]) -> Vec<f64> {
    samples.iter().map(Duration::as_secs_f64).collect()
}

/// Prints one measurement's spread and its raw samples.
fn distribution(what: &str, samples: &[f64], unit: &str) {
    if samples.is_empty() {
        return;
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!("{what}:");
    println!("  mean   {mean:.3}{unit}");
    if let Some(freedom) = samples
        .len()
        .checked_sub(BESSEL_CORRECTION)
        .filter(|&freedom| freedom > 0)
    {
        let squares: f64 = samples.iter().map(|each| (each - mean).powi(2)).sum();
        println!("  stdev  {:.3}{unit}", (squares / freedom as f64).sqrt());
    }
    println!(
        "  samples {:?}",
        samples
            .iter()
            .map(|each| format!("{each:.3}{unit}"))
            .collect::<Vec<_>>()
    );
}
