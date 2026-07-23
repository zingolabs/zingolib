//! The spawnable Nym mixnet SOCKS5 proxy process (ADR 0011, consumption
//! model A).
//!
//! The wallet bundles this binary and spawns it as a child process. On
//! startup it connects a [`NymProxy`](zingo_netutils::NymProxy) to the Nym
//! mixnet, then prints its local SOCKS5 address to stdout as a single line:
//!
//! ```text
//! SOCKS5_ADDR=127.0.0.1:43210
//! ```
//!
//! The address is not announced the instant the SOCKS5 listener is up. A
//! gateway draw can bring the listener up yet carry no data end to end — the
//! tunnel establishes but a TLS handshake over it stalls — and announcing then
//! would make the wallet mark Mixnet Mode ready against a dead path, so every
//! send fails closed. Instead the binary health-gates readiness: it runs a real
//! `GetLightdInfo` round trip through the mixnet, and only on success prints the
//! address. On failure it redraws a fresh set of gateways and retries, and if
//! the attempts exhaust it exits non-zero so the supervisor records the proxy
//! as died rather than ready.
//!
//! The parent reads the announced line to learn where to dial, then routes send
//! and price-fetch traffic through it. The process serves until either it is
//! interrupted (`Ctrl-C` for a standalone run) or its stdin closes — the
//! signal that the parent wallet has gone, since the supervisor holds that
//! pipe open for the child's whole life. On either it disconnects from the
//! mixnet cleanly. The stdin watchdog is what guarantees no orphaned proxy
//! outlives its parent, even a parent killed with `SIGKILL`. Startup failures
//! are reported on stderr with a non-zero exit so the parent can surface a
//! Mixnet Mode error rather than silently falling back to clearnet.
//!
//! This binary builds only with the `nym` feature and only in this crate's
//! own lockfile, where the nym-sdk stack resolves independently of the
//! parent workspace's crypto-common pin.
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use zingo_netutils::{
    NYM_STATUS_LINE_PREFIX, NymProxy, SOCKS5_ADDR_LINE_PREFIX, get_lightd_info_via_socks5,
};

/// The indexer contacted to prove the mixnet actually carries data before the
/// proxy announces readiness. Reached only over the mixnet, and `GetLightdInfo`
/// carries no wallet data, so this leaks nothing about the user. A widely
/// deployed, reliable mainnet endpoint; contacting it is a health probe, not a
/// wallet operation. (A future refinement could rotate this across a set.)
const HEALTH_CHECK_INDEXER: &str = "https://zec.rocks:443";

/// Per-attempt bound on the health round trip. A dead path stalls the TLS
/// handshake, so this must fire well within the mixnet's own lifecycle cap.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// How many draws to try before giving up. Each failure redraws a fresh set of
/// gateways, so this is the number of distinct mixnet paths attempted.
const MAX_HEALTH_ATTEMPTS: usize = 3;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nym-proxy: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Narrate the bootstrap on stdout so the parent supervisor can surface
    // live progress (`nym status`) instead of an opaque wait.
    let mut proxy = NymProxy::start_with_progress(|line| {
        println!("{NYM_STATUS_LINE_PREFIX}{line}");
        let _ = std::io::stdout().flush();
    })
    .await?;

    // Health-gate readiness: prove the mixnet carries data end to end before
    // announcing, redrawing gateways on failure. Only a verified path is
    // announced.
    health_gate(&mut proxy).await?;

    // Announce the address on a single line and flush, so the parent sees it
    // the moment the mixnet is verified reachable.
    println!("{SOCKS5_ADDR_LINE_PREFIX}{}", proxy.socks5_addr());
    std::io::stdout().flush()?;

    // Serve until either the parent goes away (stdin closes — the durable
    // coupling that survives even a SIGKILL of the parent) or an interrupt
    // arrives (Ctrl-C for a standalone run). Then disconnect cleanly.
    tokio::select! {
        _ = wait_for_parent_exit() => {}
        result = tokio::signal::ctrl_c() => { result?; }
    }
    proxy.disconnect().await;
    Ok(())
}

/// Prove the mixnet carries data end to end, redrawing gateways until it does
/// or the attempts exhaust. Each attempt runs a real `GetLightdInfo` round trip
/// through the local SOCKS5 tunnel — the exact path a send takes, and the one a
/// dead draw stalls at the TLS handshake — rather than a bare tunnel-establish
/// check, which a dead-data-path draw would pass. Progress is narrated on
/// stdout so `nym status` shows the verification. Returns an error only when
/// every draw fails, which the caller turns into a non-zero exit.
async fn health_gate(proxy: &mut NymProxy) -> Result<(), Box<dyn std::error::Error>> {
    let indexer: http::Uri = HEALTH_CHECK_INDEXER.parse()?;
    for attempt in 1..=MAX_HEALTH_ATTEMPTS {
        report(format!(
            "verifying the mixnet path (attempt {attempt}/{MAX_HEALTH_ATTEMPTS})"
        ));
        match get_lightd_info_via_socks5(&proxy.socks5_addr(), &indexer, HEALTH_CHECK_TIMEOUT).await
        {
            Ok(_) => {
                report("mixnet path verified".to_string());
                return Ok(());
            }
            Err(e) if attempt < MAX_HEALTH_ATTEMPTS => {
                report(format!("mixnet path unverified ({e}); redrawing gateways"));
                proxy.reconnect().await?;
            }
            Err(e) => {
                return Err(format!(
                    "the mixnet path failed verification after {MAX_HEALTH_ATTEMPTS} draws: {e}"
                )
                .into());
            }
        }
    }
    // The loop returns on the final attempt; this is unreachable but keeps the
    // function total without an explicit panic.
    Err("health check exhausted".into())
}

/// Emit a bootstrap status line on stdout, flushed, so the supervisor's live
/// `nym status` detail updates in step with the verification.
fn report(line: String) {
    println!("{NYM_STATUS_LINE_PREFIX}{line}");
    let _ = std::io::stdout().flush();
}

/// Resolves when stdin reaches EOF, which happens when the parent closes its
/// end of the pipe — on a clean exit, a panic, or a SIGKILL. Any read error is
/// also treated as "parent gone". Bytes on stdin are ignored: the pipe's
/// openness, not its content, is the signal. For a standalone run stdin is the
/// terminal, which never reaches EOF, so this simply never resolves and
/// Ctrl-C drives shutdown instead.
async fn wait_for_parent_exit() {
    let mut stdin = tokio::io::stdin();
    let mut scratch = [0u8; 64];
    loop {
        match stdin.read(&mut scratch).await {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}
