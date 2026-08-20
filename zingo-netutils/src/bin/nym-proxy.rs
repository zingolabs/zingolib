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

/// Why the proxy process stopped short of serving.
#[derive(Debug, thiserror::Error)]
enum ProxyExit {
    /// The argument grammar refused what the parent passed.
    #[error(transparent)]
    Arguments(#[from] ArgumentsError),
    /// The mixnet refused the discovery or the bootstrap.
    #[error(transparent)]
    Nym(#[from] zingo_netutils::NymProxyError),
    /// The interrupt handler could not be installed.
    #[error("the interrupt handler failed")]
    Interrupt(#[source] std::io::Error),
}

async fn run() -> Result<(), ProxyExit> {
    let arguments = parse_arguments(std::env::args().skip(1))?;

    // The parent's one window onto the exit directory: it cannot query the
    // Nym API itself, since the nym stack resolves only in this lockfile.
    if arguments.discover {
        for exit_node in NymProxy::discover_exit_nodes().await? {
            emit(format!("{NYM_EXIT_LINE_PREFIX}{exit_node}"));
        }
        return Ok(());
    }

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
        result = tokio::signal::ctrl_c() => { result.map_err(ProxyExit::Interrupt)?; }
    }
    proxy.disconnect().await;
    Ok(())
}

/// Bootstrap the proxy over the parent-supplied clutch (or a self-drawn one
/// for a standalone run), then announce the bound Exit Node and the SOCKS5
/// address at bind time.
async fn bootstrap(arguments: Arguments) -> Result<NymProxy, ProxyExit> {
    // Narrate the bootstrap on stdout so the parent supervisor can surface
    // live progress (`nym status`) instead of an opaque wait.
    let narrate = |line: String| emit(format!("{NYM_STATUS_LINE_PREFIX}{line}"));
    let proxy = if arguments.clutch.is_empty() {
        NymProxy::start().await?
    } else {
        NymProxy::start_over(arguments.clutch, narrate).await?
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
#[derive(Debug)]
struct Arguments {
    /// The clutch of Exit Node Reservations the parent drew for this
    /// acquisition; empty means draw one locally.
    clutch: Vec<String>,
    /// Whether to print the discovered Exit Nodes and exit instead of
    /// bootstrapping, the parent's one window onto the directory.
    discover: bool,
}

/// A refusal of the proxy binary's argument grammar.
#[derive(Debug, thiserror::Error)]
enum ArgumentsError {
    /// `--exit` arrived without an Exit Node identity.
    #[error("--exit needs an Exit Node identity")]
    MissingExitIdentity,
    /// An argument outside the grammar.
    #[error("unknown argument: {argument}")]
    UnknownArgument {
        /// The argument the grammar does not know.
        argument: String,
    },
}

/// Parse every `--exit <identity>` pair and the optional `--discover` flag
/// from `args`, refusing unknown arguments.
fn parse_arguments(mut args: impl Iterator<Item = String>) -> Result<Arguments, ArgumentsError> {
    let mut clutch = Vec::new();
    let mut discover = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--discover" => discover = true,
            "--exit" => match args.next() {
                Some(identity) => clutch.push(identity),
                None => return Err(ArgumentsError::MissingExitIdentity),
            },
            other => {
                return Err(ArgumentsError::UnknownArgument {
                    argument: other.to_string(),
                });
            }
        }
    }
    Ok(Arguments { clutch, discover })
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
    use super::parse_arguments;

    fn parse(args: &[&str]) -> Result<super::Arguments, super::ArgumentsError> {
        parse_arguments(args.iter().map(ToString::to_string))
    }

    /// HYPOTHESIS: each grammar refusal is a distinct typed variant, so a
    /// wrapper matches the refusal instead of parsing prose. Falsified if a
    /// refusal loses its variant or its payload.
    #[test]
    fn refusals_are_typed_variants() {
        assert!(matches!(
            parse(&["--exit"]).unwrap_err(),
            super::ArgumentsError::MissingExitIdentity
        ));
        assert!(matches!(
            parse(&["--bogus"]).unwrap_err(),
            super::ArgumentsError::UnknownArgument { argument } if argument == "--bogus"
        ));
    }

    /// HYPOTHESIS: the retired `--responsiveness` flag refuses as an unknown
    /// argument, so a version-skewed older parent is diagnosed loudly rather
    /// than silently accepted.
    #[test]
    fn the_retired_class_flag_refuses_as_unknown() {
        assert!(matches!(
            parse(&["--responsiveness", "prioritise-speed"]).unwrap_err(),
            super::ArgumentsError::UnknownArgument { argument }
                if argument == "--responsiveness"
        ));
    }

    /// HYPOTHESIS: the clutch grammar accepts repeated `--exit` pairs and
    /// refuses anything else, so a malformed spawn fails loudly instead of
    /// silently racing a short clutch.
    #[test]
    fn the_clutch_grammar_is_pairs_only() {
        assert_eq!(
            parse(&[]).expect("no arguments, no clutch").clutch,
            Vec::<String>::new()
        );
        assert_eq!(
            parse(&["--exit", "id-a", "--exit", "id-b"])
                .expect("two well-formed pairs")
                .clutch,
            vec!["id-a".to_string(), "id-b".to_string()]
        );
        assert!(parse(&["--exit"]).is_err(), "a dangling flag refuses");
        assert!(
            parse(&["--unknown"]).is_err(),
            "an unknown argument refuses"
        );
    }

    /// HYPOTHESIS: `--discover` is a standalone flag that composes with the
    /// rest of the grammar, and is off unless named.
    #[test]
    fn the_discover_flag_is_off_unless_named() {
        assert!(!parse(&[]).expect("bare invocation").discover);
        assert!(parse(&["--discover"]).expect("the flag alone").discover);
    }
}
