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
//! An arm is a commit and a build kind, written `<commit>[:clearnet]`. The
//! default kind builds the CLI mixnet-capable and bundles the proxy, so an
//! Online session spawns it, proves the quartet, and runs the
//! Server-Selection Sweep beside the scan; `:clearnet` builds without the
//! mixnet and bundles nothing. Sync itself rides clearnet in both kinds,
//! since the sync engine takes a plain gRPC indexer and no SOCKS5 route, so
//! one commit against itself in the two kinds measures what the mixnet's
//! presence costs a clearnet sync rather than what a mixnet carrier costs.
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
//! The wallet is the caller's too. `--seed` restores a named mnemonic in
//! every arm, defaulting to a fundless one. A wallet with history decrypts
//! notes where a fundless one only discards, so reproducing someone's
//! measurement means giving the comparison their wallet.
//!
//! Usage: `makers sync-ab [cli] <arm> <arm> --birthday <height>
//! [--runs <n>] [--seconds <n>] [--seed <mnemonic>]`. The reserved first word names the session
//! kind, as `makers test packages` and `makers test live` name their
//! scopes, and omitting it names the same kind.
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// The session kind a reserved first word names, following the house idiom
/// that gives `makers test` its `packages` and `live` words.
// One kind exists here, and the word still earns its place: an in-process
// harness is a second kind, and naming this one now lets that one arrive as
// another word rather than as a redesign of the grammar.
const CLI_SESSION: &str = "cli";

/// The suffix marking an arm built without the mixnet.
const CLEARNET_KIND: &str = ":clearnet";

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

/// The unit suffix the closing marker's duration carries.
const MILLIS_SUFFIX: &str = "ms";

/// The log filter the session runs under, since an empty one silences the
/// markers this reads.
const LOG_FILTER: &str = "info";

/// The indexer every run pins, so no arm meets a different server.
const PINNED_INDEXER: &str = "https://zec.rocks:443";

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

/// How far the arms' output counts may differ before the comparison is
/// reported as drifted: the tip advances during a run, so a later run scans
/// a few more outputs than an earlier one.
const OUTPUT_DRIFT_TOLERANCE: f64 = 0.01;

fn main() {
    run("sync-ab", compare, |()| {})
}

/// A commit and the build kind it is measured in.
#[derive(Clone, PartialEq, Eq)]
struct Spec {
    commit: String,
    clearnet: bool,
}

impl Spec {
    /// Reads `<commit>[:clearnet]`.
    fn parse(raw: &str) -> Self {
        match raw.strip_suffix(CLEARNET_KIND) {
            Some(commit) => Spec {
                commit: commit.to_string(),
                clearnet: true,
            },
            None => Spec {
                commit: raw.to_string(),
                clearnet: false,
            },
        }
    }

    /// The arm's label, short enough to head a row.
    fn label(&self) -> String {
        let commit: String = self.commit.chars().take(12).collect();
        if self.clearnet {
            format!("{commit}{CLEARNET_KIND}")
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
}

/// One session's reading, taken from the engine's span but for the boot.
struct Reading {
    boot: Duration,
    sync: Duration,
    outputs: u64,
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
             :clearnet kind; every other parameter is fixed so the comparison \
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
    })
}

fn compare() -> Result<(), Vec<String>> {
    let request = parse_request()?;
    let root = repo_root()?;

    let mut arms = [
        prepare(&root, &request.specs[0])?,
        prepare(&root, &request.specs[1])?,
    ];

    // The warm-up pays for each arm's build and is discarded: a first run
    // reads slower than its successors, and both arms deserve the same
    // treatment.
    eprintln!(
        "sync-ab: warming up both arms at birthday {}",
        request.birthday
    );
    for arm in &arms {
        let _ = measure(arm, &request, WARMUP_BUDGET)?;
    }

    for round in 1..=request.runs {
        // The order alternates, so whatever advantage a second run enjoys
        // falls on each arm equally instead of on whichever went last.
        let order: [usize; 2] = if round % 2 == 1 { [0, 1] } else { [1, 0] };
        for index in order {
            let reading = measure(&arms[index], &request, RUN_BUDGET)?;
            eprintln!(
                "sync-ab: {round}/{} {} boot {:.1}s, {:.0} outputs/s over {} outputs",
                request.runs,
                arms[index].spec.label(),
                reading.boot.as_secs_f64(),
                reading.rate(),
                reading.outputs
            );
            let arm = &mut arms[index];
            arm.rates.push(reading.rate());
            arm.outputs.push(reading.outputs);
            arm.boots.push(reading.boot);
        }
    }

    report(&arms);
    for arm in &arms {
        retire(&root, &arm.worktree);
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
    let opens = source
        .find(SPAWN_ANCHOR)
        .expect("the anchor was just counted")
        + SPAWN_ANCHOR.len();
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
fn measure(arm: &Arm, request: &Request, budget: Duration) -> Result<Reading, Vec<String>> {
    let scratch = arm.worktree.join("target").join("sync-ab");
    // A fresh wallet each run, so no session resumes a partial scan.
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| vec![format!("cannot make {}: {e}", scratch.display())])?;
    let log_path = scratch.join("session.log");

    let mut command = launcher(arm)?;
    command
        .current_dir(&arm.worktree)
        .env("RUST_LOG", LOG_FILTER);
    if arm.consents {
        command.arg("--online");
    }
    let mut child = command
        .arg("--server")
        .arg(PINNED_INDEXER)
        .arg("--seed")
        .arg(&request.seed)
        .arg("--birthday")
        .arg(request.birthday.to_string())
        .arg("--data-dir")
        .arg(scratch.join("wallets"))
        .arg("--log-file")
        .arg(&log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| vec![format!("cannot spawn the session: {e}")])?;

    // A session that cannot consent never launches sync on its own, so it
    // is told to, the way the user does.
    if !arm.consents {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = writeln!(stdin, "sync run");
            let _ = stdin.flush();
        }
    }

    let spawned = Instant::now();
    let mut launched: Option<Instant> = None;
    let mut stopped = false;
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
                    if let Some(stdin) = child.stdin.as_mut() {
                        let _ = writeln!(stdin, "sync stop");
                        let _ = stdin.flush();
                    }
                    stopped = true;
                }
            }
        }
        if let Some((millis, outputs)) = closing_span(&log) {
            break match launched {
                Some(at) => Ok(Reading {
                    boot: at.duration_since(spawned),
                    sync: Duration::from_millis(millis),
                    outputs,
                }),
                None => Err(vec!["the span closed without opening".to_string()]),
            };
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(vec![format!("the session exited early ({status})")]);
        }
        if spawned.elapsed() >= budget {
            break Err(vec![format!("budget of {}s exceeded", budget.as_secs())]);
        }
        std::thread::sleep(LOG_POLL_INTERVAL);
    };

    quit(&mut child);
    outcome
}

/// The command that builds and launches one arm's CLI.
// An arm carrying the run-cli task is launched through it, so the mixnet
// build and the bundled proxy are exactly what that task provisions. An arm
// predating the task is built and run directly, which is the only way to
// reach the era this comparison looks back into.
fn launcher(arm: &Arm) -> Result<Command, Vec<String>> {
    if arm.hosted {
        let mut command = Command::new("makers");
        command.arg("run-cli").arg("--release");
        if arm.spec.clearnet {
            command.arg("--clearnet");
        }
        return Ok(command);
    }
    let mut build = Command::new("cargo");
    build
        .current_dir(&arm.worktree)
        .args(["build", "--release", "-p", "zingo-cli"]);
    if arm.spec.clearnet {
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

/// The engine's own duration and output count, from the closing line.
fn closing_span(log: &str) -> Option<(u64, u64)> {
    let tail = log.split(SYNC_SPAN_CLOSE).nth(1)?;
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

/// Ends the session the way a user does, then reaps it.
fn quit(child: &mut Child) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "quit");
        let _ = stdin.flush();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Prints both arms, their spread, and whether the window held.
fn report(arms: &[Arm; 2]) {
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
    }

    let counted: Vec<u64> = arms.iter().flat_map(|arm| arm.outputs.clone()).collect();
    let (low, high) = (
        counted.iter().copied().min().unwrap_or_default(),
        counted.iter().copied().max().unwrap_or_default(),
    );
    let drift = if low == 0 {
        0.0
    } else {
        (high - low) as f64 / low as f64
    };
    println!(
        "outputs scanned: {low} to {high} ({:.2}% drift, the tip advancing mid-run)",
        drift * 100.0
    );
    if drift > OUTPUT_DRIFT_TOLERANCE {
        println!(
            "  WARNING: the arms scanned materially different work, so the \
             rates are not comparable"
        );
    }

    let (left, right) = (mean(&arms[0].rates), mean(&arms[1].rates));
    let gap = if left == 0.0 {
        0.0
    } else {
        (right - left) / left * 100.0
    };
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
    fn the_graft_surrounds_each_era_without_touching_its_call() {
        for era in [EARLY_ERA, LATER_ERA] {
            let grafted = graft(era).expect("the era anchors exactly once");
            assert!(
                grafted.contains(SYNC_SPAN_OPEN) && grafted.contains(SYNC_SPAN_CLOSE),
                "the span must open and close: {grafted}"
            );
            assert!(
                grafted.contains(SYNC_SPAN_OUTPUTS),
                "the close must carry the output count: {grafted}"
            );
            let call = era
                .split_once(SPAWN_ANCHOR)
                .expect("the era anchors")
                .1
                .rsplit_once("});")
                .expect("the launch closes")
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
    /// keeps it. Falsified if a plain commit reads as clearnet or the
    /// suffix survives into the commit.
    #[test]
    fn an_arm_reads_its_build_kind_off_its_suffix() {
        let plain = Spec::parse("dev");
        assert_eq!(plain.commit, "dev");
        assert!(!plain.clearnet);
        let bare = Spec::parse("dev:clearnet");
        assert_eq!(bare.commit, "dev");
        assert!(bare.clearnet);
        assert_eq!(bare.label(), "dev:clearnet");
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
}
