#![forbid(unsafe_code)]

use std::sync::Mutex;
use tracing_subscriber::EnvFilter;

/// Parses CLI arguments and handles the help short-circuit.
///
/// The help check is tightly coupled with argument parsing so that the
/// two cannot be accidentally reordered in `main`.
fn parse_args_or_exit_for_help() -> clap::ArgMatches {
    let matches = zingo_cli::build_clap_app().get_matches();
    if let Some(help_text) = zingo_cli::help_output(&matches) {
        for line in help_text.lines() {
            println!("{line}");
        }
        std::process::exit(0x0100);
    }
    matches
}

/// Default log file directory.
const LOG_DIR: &str = ".zingo-cli";
/// Default log file name within the log directory.
const LOG_FILE: &str = "cli.log";

/// Initializes tracing based on the mode of operation.
///
/// In interactive mode, logs are written to a file so error-level tracing
/// output (e.g. from pepper_sync) does not pollute the REPL. In command
/// mode, logs go to stderr as before.
fn init_tracing(matches: &clap::ArgMatches) {
    let env_filter = EnvFilter::from_default_env();

    if zingo_cli::is_interactive(matches) {
        let log_path = std::path::PathBuf::from(LOG_DIR).join(LOG_FILE);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => {
                tracing_subscriber::fmt()
                    .with_env_filter(env_filter)
                    .with_writer(Mutex::new(file))
                    .with_ansi(false)
                    .init();
                return;
            }
            Err(e) => {
                eprintln!(
                    "Warning: could not open log file {}: {e}. Logging to stderr.",
                    log_path.display()
                );
            }
        }
    }

    // Command mode or file-creation fallback: log to stderr
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

pub fn main() {
    // install default crypto provider (ring)
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {e:?}");
    }
    let matches = parse_args_or_exit_for_help();
    init_tracing(&matches);
    zingo_cli::run_cli(matches);
}
