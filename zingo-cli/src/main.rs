#![forbid(unsafe_code)]

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

pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    // install default crypto provider (ring)
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {e:?}");
    }
    let matches = parse_args_or_exit_for_help();
    zingo_cli::run_cli(matches);
}
