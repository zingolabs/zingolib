//! Tease apart a failing Server-Selection Sweep with an independent client.
//!
//! The harness spawns the bundled `nym-proxy` standalone (it draws its own
//! exit), then drives `grpcurl` — a gRPC client that shares no code with the
//! wallet — through the proxy's SOCKS5 port via an `ncat` bridge, running
//! the same `GetLatestBlock` the sweep surveys with. The layers separate:
//! an indexer that answers `grpcurl` over clearnet but not through the
//! bridge indicts the proxy, the exit, or the tunnel, never the wallet's
//! client code; a bridge that answers where the sweep reported silence
//! indicts the sweep's own fan-out.
//!
//! Usage: `sweep-teardown [--rounds N] [--proxy <path>]`. Requires `grpcurl`
//! and `ncat` on PATH, and by default the debug-profile bundled proxy.

#![forbid(unsafe_code)]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// The local port the ncat bridge listens on for grpcurl.
const BRIDGE_PORT: u16 = 19080;

/// The reflection-capable census members the harness probes.
const HOSTS: [&str; 3] = ["zec.rocks", "na.zec.rocks", "eu.zec.stardust.rest"];

/// The RPC every sweep probe wraps.
const RPC: &str = "cash.z.wallet.sdk.rpc.CompactTxStreamer/GetLatestBlock";

/// How long the proxy may bootstrap before the harness gives up.
const BOOTSTRAP_BUDGET: Duration = Duration::from_secs(150);

/// How long one bridged probe may take: a hand-copied mirror of the
/// sweep's `PROBE_LEG_TIMEOUT` (this dependency-free workspace cannot
/// import it), retuned together with that constant.
const PROBE_BUDGET: Duration = Duration::from_secs(20);

/// How many rounds ride the same exit without `--rounds`.
const DEFAULT_ROUNDS: usize = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run("sweep-teardown", || teardown(&args), |()| {})
}

/// Runs the whole teardown, returning its findings as printed lines.
fn teardown(args: &[String]) -> Result<(), Vec<String>> {
    let rounds = parse_rounds(args)?;
    let proxy_path = parse_proxy(args)?;

    println!("spawning standalone nym-proxy (draws its own exit)...");
    let mut proxy = Command::new(&proxy_path)
        .stdin(Stdio::piped()) // held open: the proxy's parent-liveness watchdog
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| vec![format!("could not spawn {}: {e}", proxy_path.display())])?;
    let outcome = probe_rounds(&mut proxy, rounds);
    let _ = proxy.kill();
    let _ = proxy.wait();
    outcome
}

/// Waits out the proxy bootstrap, then probes every host for `rounds`
/// rounds through the announced SOCKS5 port.
fn probe_rounds(proxy: &mut Child, rounds: usize) -> Result<(), Vec<String>> {
    let stdout = proxy.stdout.take().expect("stdout was piped");
    let mut lines = std::io::BufReader::new(stdout).lines();
    let started = Instant::now();
    let mut socks_port = None;
    for line in &mut lines {
        let line = line.map_err(|e| vec![format!("proxy stdout closed: {e}")])?;
        if let Some(status) = line.strip_prefix("NYM_STATUS=") {
            println!("  … {status}");
        } else if let Some(exit) = line.strip_prefix("NYM_EXIT=") {
            println!("exit: {exit}");
        } else if let Some(addr) = line.strip_prefix("SOCKS5_ADDR=") {
            socks_port = addr.rsplit(':').next().map(str::to_string);
            break;
        }
        if started.elapsed() > BOOTSTRAP_BUDGET {
            return Err(vec!["bootstrap exceeded its budget".to_string()]);
        }
    }
    let socks_port = socks_port.ok_or_else(|| vec!["no SOCKS5_ADDR announced".to_string()])?;
    println!("proxy ready on 127.0.0.1:{socks_port}");

    for round in 1..=rounds {
        println!("--- round {round} through the SAME exit ---");
        for host in HOSTS {
            probe_host(&socks_port, host);
        }
    }
    Ok(())
}

/// Probes one host through a per-probe ncat bridge, printing the outcome.
fn probe_host(socks_port: &str, host: &str) {
    let bridge = Command::new("ncat")
        .args([
            "-lk",
            "127.0.0.1",
            &BRIDGE_PORT.to_string(),
            "--sh-exec",
            &format!("ncat --proxy 127.0.0.1:{socks_port} --proxy-type socks5 {host} 443"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut bridge) = bridge else {
        println!("  FAIL {host}: ncat did not spawn");
        return;
    };
    std::thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    let probe = Command::new("timeout")
        .args([
            &PROBE_BUDGET.as_secs().to_string(),
            "grpcurl",
            "-servername",
            host,
            "-d",
            "{}",
            &format!("127.0.0.1:{BRIDGE_PORT}"),
            RPC,
        ])
        .output();
    let elapsed = started.elapsed().as_secs_f64();
    match probe {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let height = stdout
                .lines()
                .find(|line| line.contains("height"))
                .unwrap_or("?")
                .trim();
            println!("  OK   {host}: {height} ({elapsed:.1}s)");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.lines().next().unwrap_or("no detail");
            println!("  FAIL {host}: {detail} ({elapsed:.1}s)");
        }
        Err(e) => println!("  FAIL {host}: grpcurl did not run: {e}"),
    }
    let _ = bridge.kill();
    let _ = bridge.wait();
}

/// Parses `--rounds N`, defaulting to [`DEFAULT_ROUNDS`].
fn parse_rounds(args: &[String]) -> Result<usize, Vec<String>> {
    match args.iter().position(|arg| arg == "--rounds") {
        None => Ok(DEFAULT_ROUNDS),
        Some(index) => args
            .get(index + 1)
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| vec!["--rounds takes a positive integer".to_string()]),
    }
}

/// Parses `--proxy <path>`, defaulting to the debug-profile bundled proxy.
fn parse_proxy(args: &[String]) -> Result<PathBuf, Vec<String>> {
    match args.iter().position(|arg| arg == "--proxy") {
        None => Ok(repo_root()?.join("target/debug/nym-proxy")),
        Some(index) => args
            .get(index + 1)
            .map(PathBuf::from)
            .ok_or_else(|| vec!["--proxy takes a path".to_string()]),
    }
}
