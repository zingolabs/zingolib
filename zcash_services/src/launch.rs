use std::{fs::File, io::Read as _, path::PathBuf, process::Child};

use tempfile::TempDir;

use crate::{error::LaunchError, logs, Process};

/// Wait until the process logs indicate the launch has succeeded or failed.
pub(crate) fn wait(
    process: Process,
    handle: &mut Child,
    logs_dir: &TempDir,
    additional_log_path: Option<PathBuf>,
    success_indicators: &[&str],
    error_indicators: &[&str],
    excluded_errors: &[&str],
) -> Result<(), LaunchError> {
    let stdout_log_path = logs_dir.path().join(logs::STDOUT_LOG);
    let mut stdout_log = File::open(stdout_log_path).expect("should be able to open log");
    let mut stdout = String::new();

    let stderr_log_path = logs_dir.path().join(logs::STDERR_LOG);
    let mut stderr_log = File::open(stderr_log_path).expect("should be able to open log");
    let mut stderr = String::new();

    let (mut additional_log_file, mut additional_log) = if let Some(log_path) = additional_log_path
    {
        let log_file = File::open(log_path).expect("should be able to open log");
        let log = String::new();

        (Some(log_file), Some(log))
    } else {
        (None, None)
    };

    // wait for stdout log entry that indicates daemon is ready
    let interval = std::time::Duration::from_millis(100);
    loop {
        match handle.try_wait() {
            Ok(Some(exit_status)) => {
                stdout_log.read_to_string(&mut stdout).unwrap();
                stderr_log.read_to_string(&mut stderr).unwrap();

                return Err(LaunchError::ProcessFailed {
                    process_name: process.to_string(),
                    exit_status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => (),
            Err(e) => {
                panic!("Unexpected Error: {e}")
            }
        };

        stdout_log.read_to_string(&mut stdout).unwrap();
        stderr_log.read_to_string(&mut stderr).unwrap();

        if contains_any(&stdout, success_indicators) || contains_any(&stderr, success_indicators) {
            // launch successful
            break;
        }

        let trimmed_stdout = exclude_errors(&stdout, excluded_errors);
        let trimmed_stderr = exclude_errors(&stderr, excluded_errors);
        if contains_any(&trimmed_stdout, error_indicators)
            || contains_any(&trimmed_stderr, error_indicators)
        {
            tracing::info!("\nSTDOUT:\n{}", stdout);
            if additional_log_file.is_some() {
                let mut log_file = additional_log_file
                    .take()
                    .expect("additional log exists in this scope");
                let mut log = additional_log
                    .take()
                    .expect("additional log exists in this scope");

                log_file.read_to_string(&mut log).unwrap();
                tracing::info!("\nADDITIONAL LOG:\n{}", log);
            }
            tracing::error!("\nSTDERR:\n{}", stderr);
            panic!("\n{} launch failed without reporting an error code!\nexiting with panic. you may have to shut the daemon down manually.", process);
        }

        if additional_log_file.is_some() {
            let mut log_file = additional_log_file
                .take()
                .expect("additional log exists in this scope");
            let mut log = additional_log
                .take()
                .expect("additional log exists in this scope");

            log_file.read_to_string(&mut log).unwrap();

            if contains_any(&log, success_indicators) {
                // launch successful
                break;
            }

            let trimmed_log = exclude_errors(&log, excluded_errors);
            if contains_any(&trimmed_log, error_indicators) {
                tracing::info!("\nSTDOUT:\n{}", stdout);
                tracing::info!("\nADDITIONAL LOG:\n{}", log);
                tracing::error!("\nSTDERR:\n{}", stderr);
                panic!("{} launch failed without reporting an error code!\nexiting with panic. you may have to shut the daemon down manually.", process);
            } else {
                additional_log_file = Some(log_file);
                additional_log = Some(log);
            }
        }

        std::thread::sleep(interval);
    }

    Ok(())
}

fn contains_any(log: &str, indicators: &[&str]) -> bool {
    indicators.iter().any(|indicator| log.contains(indicator))
}

fn exclude_errors(log: &str, excluded_errors: &[&str]) -> String {
    log.lines()
        .filter(|line| !contains_any(line, excluded_errors))
        .collect::<Vec<&str>>()
        .join("\n")
}
