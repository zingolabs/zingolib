//! Measures how often a drawn Exit Node is unavailable, and how long a
//! birth waits to find out.
//!
//! ADR 0043 put a quarter to a third of Nym Exit Nodes at carrying
//! nothing, and ADR 0045 leans on that rate: it decides how many lanes a
//! boot needs to fill its quartet. The rate has never been measured from
//! this workspace, and neither has the announcement latency the
//! `EXIT_ANNOUNCEMENT_GRACE` budget was chosen without.
//!
//! One trial spawns `nym-proxy` against one exit drawn from the directory,
//! waits for it to announce that exit and publish a SOCKS5 address, then
//! probes the Sentinel through it and stops it. The two measurements are
//! different failures: silence to the grace is an exit that could not be
//! reached at all, while a Sentinel that stays silent through a bound
//! address is the exit that announced and then carried nothing.
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// Agreeing representations of `zingo_netutils`'s minted stdout tokens,
/// restated because the workbench is deliberately std-only and takes no
/// production dependency; these must not drift from that crate.
const NYM_EXIT_LINE_PREFIX: &str = "NYM_EXIT=";

/// The token the proxy publishes its local SOCKS5 address under, agreeing
/// with `zingo_netutils::SOCKS5_ADDR_LINE_PREFIX`.
const SOCKS5_ADDR_LINE_PREFIX: &str = "SOCKS5_ADDR=";

/// The address the Sentinel probes, agreeing with
/// `zingo_netutils::sentinel::SENTINEL_HOST` in its four octets.
const SENTINEL_HOST_OCTETS: [u8; 4] = [1, 1, 1, 1];

/// The port the Sentinel probes, agreeing with
/// `zingo_netutils::sentinel::SENTINEL_PORT`.
const SENTINEL_PORT: u16 = 53;

/// How long one Sentinel round trip may take before the exit is judged to
/// carry nothing, agreeing with `zingo_netutils::time::SENTINEL_BUDGET`.
const SENTINEL_BUDGET: Duration = Duration::from_millis(3_500);

/// The reply bytes a probe reads before it stops, agreeing with the
/// production Sentinel's read.
const SENTINEL_READ_BYTES: usize = 64;

/// The transaction identifier every Sentinel query carries, agreeing with
/// the production Sentinel's fixed identifier.
const SENTINEL_QUERY_ID: [u8; 2] = [0xAB, 0xCD];

/// The DNS header following the identifier: a standard recursive query
/// carrying one question and no other records.
const SENTINEL_QUERY_HEADER: [u8; 10] =
    [0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

/// The question's trailer: the root label, then type `A` in class `IN`.
const SENTINEL_QUESTION_TRAILER: [u8; 5] = [0x00, 0x00, 0x01, 0x00, 0x01];

/// The name the Sentinel looks up, which names nothing the wallet wants.
const SENTINEL_QUERY_LABELS: [&str; 2] = ["example", "com"];

/// The SOCKS5 protocol version every greeting and request opens with.
const SOCKS5_VERSION: u8 = 0x05;

/// The SOCKS5 authentication method the proxy offers, meaning none.
const SOCKS5_NO_AUTH: u8 = 0x00;

/// How many authentication methods the greeting offers, which is the one.
const SOCKS5_METHODS_OFFERED: u8 = 1;

/// The SOCKS5 command that opens an outbound connection.
const SOCKS5_CONNECT: u8 = 0x01;

/// The SOCKS5 reserved byte, which every request sets to zero.
const SOCKS5_RESERVED: u8 = 0x00;

/// The SOCKS5 address type naming a literal IPv4 destination.
const SOCKS5_ATYP_IPV4: u8 = 0x01;

/// The SOCKS5 reply code meaning the connection succeeded.
const SOCKS5_SUCCEEDED: u8 = 0x00;

/// The byte count of the SOCKS5 method-selection reply.
const SOCKS5_GREETING_REPLY_BYTES: usize = 2;

/// The byte count of a SOCKS5 reply carrying a bound IPv4 address.
const SOCKS5_IPV4_REPLY_BYTES: usize = 10;

/// How long one birth may take to announce its exit before the trial
/// counts it unreachable, agreeing with
/// `zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE`.
const ANNOUNCEMENT_GRACE: Duration = Duration::from_millis(7_000);

/// How many births one trial run makes when `--births` names no other count.
const DEFAULT_TRIALS: usize = 100;

/// How often the reader wakes to check whether the grace has elapsed.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// The degree of freedom a sample standard deviation gives up to the mean
/// it is measured around.
const BESSEL_CORRECTION: usize = 1;

fn main() {
    run("birth-trial", trial, |()| {})
}

/// One birth's outcome: the exit announced and carried the Sentinel round
/// trip, announced and carried nothing, exited before announcing, or stayed
/// silent to the grace.
enum Outcome {
    Carried {
        announced: Duration,
        round_trip: Duration,
    },
    CarriesNothing {
        announced: Duration,
    },
    Exited {
        elapsed: Duration,
    },
    Unreachable,
}

/// The count of births to make, from `--births N` or the default.
fn trials_requested() -> Result<usize, Vec<String>> {
    let mut args = std::env::args().skip(1);
    let mut trials = DEFAULT_TRIALS;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--births" => {
                let count = args
                    .next()
                    .ok_or_else(|| vec!["--births needs a count".to_string()])?;
                trials = count
                    .parse()
                    .map_err(|e| vec![format!("--births {count}: {e}")])?;
            }
            other => return Err(vec![format!("unknown argument: {other}")]),
        }
    }
    Ok(trials)
}

fn trial() -> Result<(), Vec<String>> {
    let trials = trials_requested()?;
    let root = repo_root()?;
    let proxy = root.join("target").join("debug").join("nym-proxy");
    if !proxy.exists() {
        return Err(vec![format!(
            "no nym-proxy at {}: build it with `makers bundle-nym-proxy`",
            proxy.display()
        )]);
    }

    let exits = discover(&proxy)?;
    eprintln!("birth-trial: {} exits advertised", exits.len());
    if exits.len() < trials {
        return Err(vec![format!(
            "only {} exits advertised, fewer than the {trials} trials: \
             a trial would draw one twice",
            exits.len()
        )]);
    }

    let mut announced: Vec<Duration> = Vec::new();
    let mut round_trips: Vec<Duration> = Vec::new();
    let mut carries_nothing = 0usize;
    let mut exited = 0usize;
    let mut unreachable = 0usize;
    for (index, exit) in exits.iter().take(trials).enumerate() {
        match birth(&proxy, exit)? {
            Outcome::Carried {
                announced: at,
                round_trip,
            } => {
                eprintln!(
                    "birth-trial: {:3}/{trials} announced in {}ms, carried in {}ms",
                    index + 1,
                    at.as_millis(),
                    round_trip.as_millis()
                );
                announced.push(at);
                round_trips.push(round_trip);
            }
            Outcome::CarriesNothing { announced: at } => {
                eprintln!(
                    "birth-trial: {:3}/{trials} announced in {}ms, then carried nothing",
                    index + 1,
                    at.as_millis()
                );
                announced.push(at);
                carries_nothing += 1;
            }
            Outcome::Exited { elapsed } => {
                eprintln!(
                    "birth-trial: {:3}/{trials} exited after {}ms without announcing",
                    index + 1,
                    elapsed.as_millis()
                );
                exited += 1;
            }
            Outcome::Unreachable => {
                eprintln!("birth-trial: {:3}/{trials} unreachable", index + 1);
                unreachable += 1;
            }
        }
    }
    report(
        &announced,
        &round_trips,
        carries_nothing,
        exited,
        unreachable,
    );
    Ok(())
}

/// Every Exit Node the directory advertises, in the order discovery gave
/// them, which the proxy already shuffled.
fn discover(proxy: &Path) -> Result<Vec<String>, Vec<String>> {
    let output = Command::new(proxy)
        .arg("--discover")
        .output()
        .map_err(|e| vec![format!("cannot run {}: {e}", proxy.display())])?;
    if !output.status.success() {
        return Err(vec![format!("discovery failed ({})", output.status)]);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix(NYM_EXIT_LINE_PREFIX))
        .map(str::to_string)
        .collect())
}

/// Spawns one proxy pinned to `exit` and times how long it takes to
/// announce that exit, stopping it either way.
fn birth(proxy: &Path, exit: &str) -> Result<Outcome, Vec<String>> {
    // The proxy races its bootstrap against its own stdin closing, the
    // watchdog that stops an orphan outliving its parent. A trial that lets
    // the child inherit a closed stdin therefore measures the watchdog and
    // reports every exit unreachable, so this pipe stays open — unread and
    // unwritten — for the whole birth. Its stderr is inherited so a refusal
    // reaches the operator rather than the void.
    let mut child = Command::new(proxy)
        .arg("--exit")
        .arg(exit)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| vec![format!("cannot spawn {}: {e}", proxy.display())])?;

    let started = Instant::now();
    let stdout = child.stdout.take().expect("stdout was piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let announced = line.starts_with(NYM_EXIT_LINE_PREFIX);
            let published = line.starts_with(SOCKS5_ADDR_LINE_PREFIX);
            if (announced || published) && sender.send(line).is_err() {
                return;
            }
        }
    });

    // The exit is announced a moment before its address is published, so
    // the announcement latency is stamped when the first line arrives and
    // the probe waits for the second.
    let mut announced: Option<Duration> = None;
    let outcome = loop {
        // The published lines are read before the exit status, so a proxy
        // that publishes and then stops still counts as having published.
        match receiver.try_recv() {
            Ok(line) if line.starts_with(NYM_EXIT_LINE_PREFIX) => {
                announced = Some(started.elapsed());
                continue;
            }
            Ok(line) => {
                let at = announced.unwrap_or_else(|| started.elapsed());
                break probe(&line, at)?;
            }
            Err(_) => {}
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            break Outcome::Exited {
                elapsed: started.elapsed(),
            };
        }
        if started.elapsed() >= ANNOUNCEMENT_GRACE {
            break Outcome::Unreachable;
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let _ = child.kill();
    let _ = child.wait();
    Ok(outcome)
}

/// Probes the Sentinel through the address the proxy just published, so an
/// exit that announced and carries nothing is told from one that carries.
fn probe(published: &str, announced: Duration) -> Result<Outcome, Vec<String>> {
    let addr = published
        .strip_prefix(SOCKS5_ADDR_LINE_PREFIX)
        .ok_or_else(|| vec![format!("not a published address: {published}")])?;
    let socks5: SocketAddr = addr
        .parse()
        .map_err(|e| vec![format!("cannot parse the published address {addr}: {e}")])?;

    let started = Instant::now();
    let carried = round_trip(socks5).is_some();
    let round_trip = started.elapsed();
    // Any completed round trip proves the exit, whatever the reply said,
    // and a round trip that overran the budget is silence however it ended.
    Ok(if carried && round_trip <= SENTINEL_BUDGET {
        Outcome::Carried {
            announced,
            round_trip,
        }
    } else {
        Outcome::CarriesNothing { announced }
    })
}

/// Opens the SOCKS5 connection, sends the Sentinel query, and reads
/// whatever comes back, where any non-empty read is the proof.
fn round_trip(socks5: SocketAddr) -> Option<usize> {
    let mut stream = TcpStream::connect_timeout(&socks5, SENTINEL_BUDGET).ok()?;
    stream.set_read_timeout(Some(SENTINEL_BUDGET)).ok()?;
    stream.set_write_timeout(Some(SENTINEL_BUDGET)).ok()?;

    // Greet with the single no-authentication method and require the proxy
    // to select it.
    let offered = [SOCKS5_VERSION, SOCKS5_METHODS_OFFERED, SOCKS5_NO_AUTH];
    stream.write_all(&offered).ok()?;
    let mut selection = [0u8; SOCKS5_GREETING_REPLY_BYTES];
    stream.read_exact(&mut selection).ok()?;
    if selection != [SOCKS5_VERSION, SOCKS5_NO_AUTH] {
        return None;
    }

    let mut request = vec![
        SOCKS5_VERSION,
        SOCKS5_CONNECT,
        SOCKS5_RESERVED,
        SOCKS5_ATYP_IPV4,
    ];
    request.extend_from_slice(&SENTINEL_HOST_OCTETS);
    request.extend_from_slice(&SENTINEL_PORT.to_be_bytes());
    stream.write_all(&request).ok()?;
    let mut reply = [0u8; SOCKS5_IPV4_REPLY_BYTES];
    stream.read_exact(&mut reply).ok()?;
    if reply[1] != SOCKS5_SUCCEEDED {
        return None;
    }

    stream.write_all(&sentinel_query()).ok()?;
    let mut buffer = [0u8; SENTINEL_READ_BYTES];
    match stream.read(&mut buffer).ok()? {
        0 => None,
        read => Some(read),
    }
}

/// Builds the Sentinel's query: an ordinary `A` lookup, length-prefixed as
/// DNS over TCP requires.
fn sentinel_query() -> Vec<u8> {
    let mut body = Vec::from(SENTINEL_QUERY_ID);
    body.extend_from_slice(&SENTINEL_QUERY_HEADER);
    for label in SENTINEL_QUERY_LABELS {
        body.push(label.len() as u8);
        body.extend_from_slice(label.as_bytes());
    }
    body.extend_from_slice(&SENTINEL_QUESTION_TRAILER);
    let mut framed = Vec::from((body.len() as u16).to_be_bytes());
    framed.extend(body);
    framed
}

/// Prints the rate and the latency distribution the design was missing.
fn report(
    announced: &[Duration],
    round_trips: &[Duration],
    carries_nothing: usize,
    exited: usize,
    unreachable: usize,
) {
    let total = announced.len() + exited + unreachable;
    let unreached = exited + unreachable;
    println!("\nbirths:                {total}");
    println!("announced:             {}", announced.len());
    println!("  of those, carried:   {}", round_trips.len());
    println!("  of those, carried nothing: {carries_nothing}");
    println!("exited early:          {exited}");
    println!("silent to grace:       {unreachable}");
    if total > 0 {
        println!(
            "unreachable rate:      {:.1}%",
            100.0 * unreached as f64 / total as f64
        );
    }
    if !announced.is_empty() {
        println!(
            "carries-nothing rate:  {:.1}% of the {} that announced",
            100.0 * carries_nothing as f64 / announced.len() as f64,
            announced.len()
        );
    }
    distribution("announcement latency", announced, ANNOUNCEMENT_GRACE);
    distribution("Sentinel round trip", round_trips, SENTINEL_BUDGET);
}

/// The arithmetic mean of the sampled milliseconds.
fn mean(samples: &[u128]) -> f64 {
    samples.iter().map(|&each| each as f64).sum::<f64>() / samples.len() as f64
}

/// The sample standard deviation of the sampled milliseconds, which a
/// single sample leaves undefined.
fn deviation(samples: &[u128]) -> Option<f64> {
    let freedom = samples.len().checked_sub(BESSEL_CORRECTION)?;
    if freedom == 0 {
        return None;
    }
    let centre = mean(samples);
    let squares: f64 = samples
        .iter()
        .map(|&each| (each as f64 - centre).powi(2))
        .sum();
    Some((squares / freedom as f64).sqrt())
}

/// Prints one measurement's spread against the budget it must fit inside.
fn distribution(what: &str, samples: &[Duration], budget: Duration) {
    if samples.is_empty() {
        return;
    }
    let mut sorted: Vec<u128> = samples.iter().map(Duration::as_millis).collect();
    sorted.sort_unstable();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    println!("{what} over {} samples:", sorted.len());
    println!("  min    {}ms", sorted[0]);
    println!("  median {}ms", at(0.5));
    println!("  p90    {}ms", at(0.9));
    println!("  max    {}ms", sorted[sorted.len() - 1]);
    println!("  mean   {:.0}ms", mean(&sorted));
    match deviation(&sorted) {
        Some(spread) => println!(
            "  stdev  {spread:.0}ms (quantised by the {}ms poll)",
            POLL_INTERVAL.as_millis()
        ),
        None => println!("  stdev  undefined below two samples"),
    }
    println!(
        "  budget {}ms (what these are measured against)",
        budget.as_millis()
    );
    // The raw samples, so a reader can compute what this summary omits.
    println!("  samples {sorted:?}");
}
