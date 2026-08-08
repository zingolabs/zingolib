//! The spawnable Nym mixnet SOCKS5 proxy process (ADR 0011, consumption
//! model A).
//!
//! The wallet bundles this binary and spawns it as a child process. On
//! startup it connects a [`NymProxy`] to the Nym
//! mixnet, then prints its local SOCKS5 address to stdout as a single line:
//!
//! ```text
//! SOCKS5_ADDR=127.0.0.1:43210
//! ```
//!
//! The address is not announced the instant the SOCKS5 listener is up. A
//! gateway draw can bring the listener up yet carry no data end to end (the
//! tunnel establishes but a TLS handshake over it stalls), and announcing then
//! would make the wallet mark Mixnet Mode ready against a dead path, so every
//! send fails closed. Instead the binary health-gates readiness: it runs a real
//! `GetLightdInfo` round trip through the mixnet, and only on success prints the
//! address. On failure it redraws a fresh set of gateways and retries, and if
//! the attempts exhaust it exits non-zero so the supervisor records the proxy
//! as died rather than ready.
//!
//! The parent reads the announced line to learn where to dial, then routes send
//! and price-fetch traffic through it. The process serves until either it is
//! interrupted (`Ctrl-C` for a standalone run) or its stdin closes, the
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
use zingo_netutils::{
    NYM_EXIT_LINE_PREFIX, NYM_STATUS_LINE_PREFIX, NymProxy, SOCKS5_ADDR_LINE_PREFIX,
    get_lightd_info_via_socks5,
    indexers::MIXNET_HEALTH_INDEXER,
    responsiveness::{Critical, NonCritical, ResponsivenessClass},
    time::{MIXNET_HEALTH_DRAWS, MIXNET_ROUND_TRIP_BOUND},
};

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
    let arguments = parse_arguments(std::env::args().skip(1))?;

    // The stdin watchdog covers the bootstrap too: a parent that dies while
    // this child is still drawing gateways must take the child with it, not
    // leave an orphan to finish bootstrapping against a closed pipe.
    let proxy = tokio::select! {
        _ = wait_for_parent_exit() => return Ok(()),
        outcome = bootstrap(arguments) => outcome?,
    };

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

/// Bootstrap the proxy: connect with the exclusions under the parent's
/// responsiveness class, health-gate readiness (proving the mixnet carries
/// data end to end before announcing), then announce the bound Exit Node and
/// the SOCKS5 address together.
async fn bootstrap(arguments: Arguments) -> Result<NymProxy, Box<dyn std::error::Error>> {
    // Narrate the bootstrap on stdout so the parent supervisor can surface
    // live progress (`nym status`) instead of an opaque wait.
    let narrate = |line: String| emit(format!("{NYM_STATUS_LINE_PREFIX}{line}"));
    // The one point where the wire form re-enters the type system: each
    // class monomorphizes the same start.
    let mut proxy = match arguments.class {
        ResponsivenessClass::Critical => {
            NymProxy::start_with_progress::<Critical>(arguments.excluded, narrate).await?
        }
        ResponsivenessClass::NonCritical => {
            NymProxy::start_with_progress::<NonCritical>(arguments.excluded, narrate).await?
        }
    };

    health_gate(&mut proxy).await?;

    emit(format!("{NYM_EXIT_LINE_PREFIX}{}", proxy.exit_provider()));
    emit(format!("{SOCKS5_ADDR_LINE_PREFIX}{}", proxy.socks5_addr()));
    Ok(proxy)
}

/// Prove the mixnet carries data end to end, redrawing gateways until it does
/// or the attempts exhaust. Each attempt runs a real `GetLightdInfo` round trip
/// through the local SOCKS5 tunnel (the exact path a send takes, and the one a
/// dead draw stalls at the TLS handshake) rather than a bare tunnel-establish
/// check, which a dead-data-path draw would pass. Progress is narrated on
/// stdout so `nym status` shows the verification. Returns an error only when
/// every draw fails, which the caller turns into a non-zero exit.
async fn health_gate(proxy: &mut NymProxy) -> Result<(), Box<dyn std::error::Error>> {
    let indexer: http::Uri = MIXNET_HEALTH_INDEXER.parse()?;
    for attempt in 1..=MIXNET_HEALTH_DRAWS {
        report(format!(
            "verifying the mixnet path (attempt {attempt}/{MIXNET_HEALTH_DRAWS})"
        ));
        match get_lightd_info_via_socks5(&proxy.socks5_addr(), &indexer, MIXNET_ROUND_TRIP_BOUND)
            .await
        {
            Ok(_) => {
                report("mixnet path verified".to_string());
                return Ok(());
            }
            Err(e) if attempt < MIXNET_HEALTH_DRAWS => {
                report(format!("mixnet path unverified ({e}); redrawing gateways"));
                proxy.reconnect().await?;
            }
            Err(e) => {
                return Err(format!(
                    "the mixnet path failed verification after {MIXNET_HEALTH_DRAWS} draws: {e}"
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
    emit(format!("{NYM_STATUS_LINE_PREFIX}{line}"));
}

/// Write one line to stdout, flushed, swallowing write errors: a broken pipe
/// means the parent is gone, which the stdin watchdog turns into a clean
/// exit — a panicking `println!` must never race it onto the terminal.
fn emit(line: String) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// The parent's spawn-time instructions, parsed from the argument grammar.
struct Arguments {
    /// The Exit Nodes excluded from this proxy's draw.
    excluded: Vec<String>,
    /// The acquisition's responsiveness class; a bare invocation defaults
    /// to critical, matching a person waiting at a terminal.
    class: ResponsivenessClass,
}

/// Parse every `--exclude-exit <identity>` pair and the optional
/// `--responsiveness <critical|non-critical>` from `args`, refusing unknown
/// arguments and unknown class tokens.
fn parse_arguments(mut args: impl Iterator<Item = String>) -> Result<Arguments, String> {
    let mut excluded = Vec::new();
    let mut class = ResponsivenessClass::Critical;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exclude-exit" => match args.next() {
                Some(identity) => excluded.push(identity),
                None => return Err("--exclude-exit needs an Exit Node identity".to_string()),
            },
            "--responsiveness" => match args.next() {
                Some(token) => {
                    class = ResponsivenessClass::parse(&token)
                        .ok_or_else(|| format!("unknown responsiveness class: {token}"))?;
                }
                None => return Err("--responsiveness needs a class token".to_string()),
            },
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Arguments { excluded, class })
}

/// Resolves when stdin reaches EOF, which happens when the parent closes its
/// end of the pipe, on a clean exit, a panic, or a SIGKILL. Any read error is
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

#[cfg(test)]
mod tests {
    use super::{ResponsivenessClass, parse_arguments};

    fn parse(args: &[&str]) -> Result<super::Arguments, String> {
        parse_arguments(args.iter().map(ToString::to_string))
    }

    /// HYPOTHESIS: the argument grammar accepts repeated `--exclude-exit`
    /// pairs and refuses anything else, so a malformed spawn fails loudly
    /// instead of silently dropping an exclusion.
    #[test]
    fn the_exclusion_grammar_is_pairs_only() {
        assert_eq!(
            parse(&[]).expect("no arguments, no exclusions").excluded,
            Vec::<String>::new()
        );
        assert_eq!(
            parse(&["--exclude-exit", "id-a", "--exclude-exit", "id-b"])
                .expect("two well-formed pairs")
                .excluded,
            vec!["id-a".to_string(), "id-b".to_string()]
        );
        assert!(
            parse(&["--exclude-exit"]).is_err(),
            "a dangling flag refuses"
        );
        assert!(
            parse(&["--unknown"]).is_err(),
            "an unknown argument refuses"
        );
    }

    /// HYPOTHESIS: the class grammar accepts exactly the wire tokens of the
    /// two responsiveness classes and defaults a bare invocation to
    /// critical, so a malformed spawn fails loudly instead of silently
    /// racing under the wrong policy.
    #[test]
    fn the_class_grammar_speaks_the_wire_tokens() {
        assert_eq!(
            parse(&[]).expect("bare invocation").class,
            ResponsivenessClass::Critical,
            "a person at a terminal is waiting"
        );
        assert_eq!(
            parse(&["--responsiveness", "non-critical"])
                .expect("the non-critical token")
                .class,
            ResponsivenessClass::NonCritical
        );
        assert_eq!(
            parse(&["--responsiveness", "critical", "--exclude-exit", "id-a"])
                .expect("class and exclusions compose")
                .class,
            ResponsivenessClass::Critical
        );
        assert!(
            parse(&["--responsiveness", "urgent"]).is_err(),
            "an unknown class token refuses"
        );
        assert!(
            parse(&["--responsiveness"]).is_err(),
            "a dangling flag refuses"
        );
    }
}
