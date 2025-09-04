//! Crate level error module

/// Errors associated with launching processes
#[derive(thiserror::Error, Debug, Clone)]
pub enum LaunchError {
    /// Process failed during launch
    #[error(
        "{process_name} failed during launch.\nExit status: {exit_status}\nStdout: {stdout}\nStderr: {stderr}"
    )]
    ProcessFailed {
        /// Process name
        process_name: String,
        /// Exit status
        exit_status: std::process::ExitStatus,
        /// Stdout log
        stdout: String,
        /// Stderr log
        stderr: String,
    },
}
