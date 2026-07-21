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
//! The parent reads that line to learn where to dial, then routes send and
//! price-fetch traffic through it. The process serves until either it is
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

use tokio::io::AsyncReadExt as _;
use zingo_netutils::{NYM_STATUS_LINE_PREFIX, NymProxy, SOCKS5_ADDR_LINE_PREFIX};

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
    let proxy = NymProxy::start_with_progress(|line| {
        println!("{NYM_STATUS_LINE_PREFIX}{line}");
        let _ = std::io::stdout().flush();
    })
    .await?;

    // Announce the address on a single line and flush, so the parent sees it
    // the moment the mixnet is reachable.
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
