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
//! The bound Exit Node and the address are announced the moment the bind
//! completes; end-to-end verification belongs to the session's sweep, and
//! every later Transmission doubles as a probe.
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
    responsiveness::{PrioritisePrivacy, PrioritiseSpeed, ResponsivenessClass},
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

/// Bootstrap the proxy under the parent's responsiveness class, then
/// announce the bound Exit Node and the SOCKS5 address at bind time.
async fn bootstrap(arguments: Arguments) -> Result<NymProxy, Box<dyn std::error::Error>> {
    // Narrate the bootstrap on stdout so the parent supervisor can surface
    // live progress (`nym status`) instead of an opaque wait.
    let narrate = |line: String| emit(format!("{NYM_STATUS_LINE_PREFIX}{line}"));
    // The one point where the wire form re-enters the type system: each
    // class monomorphizes the same start.
    let proxy = match arguments.class {
        ResponsivenessClass::PrioritiseSpeed => {
            NymProxy::start_with_progress::<PrioritiseSpeed>(arguments.excluded, narrate).await?
        }
        ResponsivenessClass::PrioritisePrivacy => {
            NymProxy::start_with_progress::<PrioritisePrivacy>(arguments.excluded, narrate).await?
        }
    };

    emit(format!("{NYM_EXIT_LINE_PREFIX}{}", proxy.exit_node()));
    emit(format!("{SOCKS5_ADDR_LINE_PREFIX}{}", proxy.socks5_addr()));
    Ok(proxy)
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
    /// to prioritise-speed, matching a person waiting at a terminal.
    class: ResponsivenessClass,
}

/// Parse every `--exclude-exit <identity>` pair and the optional
/// `--responsiveness <prioritise-speed|prioritise-privacy>` from `args`,
/// refusing unknown arguments and unknown class tokens.
fn parse_arguments(mut args: impl Iterator<Item = String>) -> Result<Arguments, String> {
    let mut excluded = Vec::new();
    let mut class = ResponsivenessClass::PrioritiseSpeed;
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
    /// prioritise-speed, so a malformed spawn fails loudly instead of
    /// silently racing under the wrong policy.
    #[test]
    fn the_class_grammar_speaks_the_wire_tokens() {
        assert_eq!(
            parse(&[]).expect("bare invocation").class,
            ResponsivenessClass::PrioritiseSpeed,
            "a person at a terminal is waiting"
        );
        assert_eq!(
            parse(&["--responsiveness", "prioritise-privacy"])
                .expect("the prioritise-privacy token")
                .class,
            ResponsivenessClass::PrioritisePrivacy
        );
        assert_eq!(
            parse(&[
                "--responsiveness",
                "prioritise-speed",
                "--exclude-exit",
                "id-a"
            ])
            .expect("class and exclusions compose")
            .class,
            ResponsivenessClass::PrioritiseSpeed
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
