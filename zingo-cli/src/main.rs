#![forbid(unsafe_code)]

/// Parses CLI arguments and handles the help short-circuit.
///
/// The help check is tightly coupled with argument parsing so that the
/// two cannot be accidentally reordered in `main`.
fn parse_args_or_exit_for_help() -> clap::ArgMatches {
    // Catch a session option placed after the command before clap rejects it
    // with an opaque "unexpected argument": name the misplacement and the fix.
    let args: Vec<String> = std::env::args().collect();
    if let Some(guidance) = zingo_cli::misplaced_session_option(&args) {
        eprintln!("error: {guidance}");
        std::process::exit(0x0002);
    }
    let matches = zingo_cli::build_clap_app().get_matches();
    if let Some(help_text) = zingo_cli::help_output(&matches) {
        for line in help_text.lines() {
            println!("{line}");
        }
        std::process::exit(0x0100);
    }
    matches
}

#[cfg(target_os = "linux")]
/// Reports permission diagnostics to stderr. Only tested against Linux.
fn report_permission_error() {
    let user = std::env::var("USER").expect("Unexpected error reading value of $USER!");
    let home = std::env::var("HOME").expect("Unexpected error reading value of $HOME!");
    let current_executable =
        std::env::current_exe().expect("Unexpected error reporting executable path!");
    eprintln!("USER: {user}");
    eprintln!("HOME: {home}");
    eprintln!("Executable: {}", current_executable.display());
    if home == "/" {
        eprintln!("User {user} must have permission to write to '{home}.zcash/' .");
    } else {
        eprintln!("User {user} must have permission to write to '{home}/.zcash/' .");
    }
}

fn handle_error(e: std::io::Error) {
    eprintln!("Error: {e}");
    #[cfg(target_os = "linux")]
    if let Some(13) = e.raw_os_error() {
        report_permission_error();
    }
}

pub fn main() -> std::process::ExitCode {
    zingolib::netutils::ensure_default_crypto_provider();
    let matches = parse_args_or_exit_for_help();
    zingo_cli::init_tracing(&matches);
    match zingo_cli::run_cli(matches) {
        Ok(code) => code,
        Err(e) => {
            handle_error(e);
            std::process::ExitCode::FAILURE
        }
    }
}
