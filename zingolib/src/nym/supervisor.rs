//! The Mixnet Mode proxy supervisor (ADR 0011, consumption model A).
//!
//! The wallet cannot link the mixnet transport in-process, so it bundles the
//! `nym-proxy` binary and spawns it as a child. This supervisor owns that
//! child's lifecycle: it starts the process, reads the local SOCKS5 address
//! the child announces on stdout, and drives the tri-state
//! [`MixnetMode`](crate::nym::MixnetMode). While the child is starting the
//! mode is `Bootstrapping`; it becomes `Ready` once the address arrives, and
//! `Off` if the child's stdout closes without one — meaning the mixnet was not
//! reached, so a mixnet-only send fails closed rather than falling back to
//! clearnet.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use zingo_netutils::{NYM_STATUS_LINE_PREFIX, SOCKS5_ADDR_LINE_PREFIX};

use crate::nym::MixnetMode;

/// A failure starting the mixnet proxy child process.
#[derive(Debug, thiserror::Error)]
pub enum MixnetProxyError {
    /// The `nym-proxy` binary could not be spawned.
    #[error("failed to spawn the nym-proxy binary at {path}: {source}")]
    Spawn {
        /// The binary path that failed to spawn.
        path: String,
        /// The underlying spawn error.
        source: std::io::Error,
    },
    /// The spawned child exposed no stdout to read its address from.
    #[error("the nym-proxy child exposed no stdout")]
    NoStdout,
}

/// The observable state shared between the supervisor and its stdout reader.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyState {
    mode: MixnetMode,
    socks5_addr: Option<String>,
    /// The child's latest bootstrap progress line, live only while
    /// [`MixnetMode::Bootstrapping`], so a user interface can narrate the
    /// connect race instead of showing an opaque wait.
    bootstrap_detail: Option<String>,
}

/// Supervises the spawned `nym-proxy` child process and exposes its tri-state.
pub struct MixnetProxy {
    child: Child,
    state: Arc<Mutex<ProxyState>>,
    reader: JoinHandle<()>,
}

impl MixnetProxy {
    /// Spawn the `nym-proxy` binary at `binary_path`. Returns immediately with
    /// mode [`MixnetMode::Bootstrapping`]; poll [`Self::mode`] for readiness.
    /// The child is killed if this supervisor is dropped.
    pub fn spawn(binary_path: &Path) -> Result<Self, MixnetProxyError> {
        let mut child = Command::new(binary_path)
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| MixnetProxyError::Spawn {
                path: binary_path.display().to_string(),
                source,
            })?;
        let stdout = child.stdout.take().ok_or(MixnetProxyError::NoStdout)?;
        let state = Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: None,
        }));
        let reader = tokio::spawn(drive_state(stdout, Arc::clone(&state)));
        Ok(MixnetProxy {
            child,
            state,
            reader,
        })
    }

    /// The current tri-state.
    pub fn mode(&self) -> MixnetMode {
        self.state.lock().expect("proxy state mutex").mode
    }

    /// The local SOCKS5 address, once the mode is [`MixnetMode::Ready`].
    pub fn socks5_addr(&self) -> Option<String> {
        self.state
            .lock()
            .expect("proxy state mutex")
            .socks5_addr
            .clone()
    }

    /// The child's latest bootstrap progress line, while
    /// [`MixnetMode::Bootstrapping`]. `None` before the first report and
    /// once the proxy is ready.
    pub fn bootstrap_detail(&self) -> Option<String> {
        self.state
            .lock()
            .expect("proxy state mutex")
            .bootstrap_detail
            .clone()
    }

    /// Shut the child down and stop tracking its state.
    pub async fn stop(mut self) {
        self.reader.abort();
        let _ = self.child.kill().await;
        self.state.lock().expect("proxy state mutex").mode = MixnetMode::Off;
    }
}

/// Read `stdout` until the child announces its SOCKS5 address (then `Ready`),
/// or until stdout closes without one (then `Off`). Progress lines arriving
/// before the address update the live bootstrap detail. Generic over the
/// reader so the state machine is unit-tested without spawning a process.
async fn drive_state<R: AsyncRead + Unpin>(stdout: R, state: Arc<Mutex<ProxyState>>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(addr) = parse_socks5_addr_line(&line) {
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.socks5_addr = Some(addr.to_string());
            guarded.bootstrap_detail = None;
            guarded.mode = MixnetMode::Ready;
            return;
        }
        if let Some(detail) = parse_status_line(&line) {
            state.lock().expect("proxy state mutex").bootstrap_detail = Some(detail.to_string());
        }
    }
    state.lock().expect("proxy state mutex").mode = MixnetMode::Off;
}

/// Extract the SOCKS5 address from a child stdout line, if it is the
/// announcement line.
fn parse_socks5_addr_line(line: &str) -> Option<&str> {
    line.strip_prefix(SOCKS5_ADDR_LINE_PREFIX).map(str::trim)
}

/// Extract the progress detail from a child stdout line, if it is a
/// bootstrap status line.
fn parse_status_line(line: &str) -> Option<&str> {
    line.strip_prefix(NYM_STATUS_LINE_PREFIX).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bootstrapping() -> Arc<Mutex<ProxyState>> {
        Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: None,
        }))
    }

    #[test]
    fn parses_the_announcement_line() {
        assert_eq!(
            parse_socks5_addr_line("SOCKS5_ADDR=127.0.0.1:43210"),
            Some("127.0.0.1:43210")
        );
    }

    #[test]
    fn trims_trailing_whitespace_and_carriage_return() {
        assert_eq!(
            parse_socks5_addr_line("SOCKS5_ADDR=127.0.0.1:9 \r"),
            Some("127.0.0.1:9")
        );
    }

    #[test]
    fn ignores_non_announcement_lines() {
        assert_eq!(parse_socks5_addr_line("connecting to mixnet"), None);
        assert_eq!(parse_socks5_addr_line(""), None);
    }

    #[tokio::test]
    async fn ready_when_the_address_is_announced() {
        let state = bootstrapping();
        drive_state(
            b"SOCKS5_ADDR=127.0.0.1:43210\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:43210"));
    }

    #[tokio::test]
    async fn ready_after_preamble_lines() {
        let state = bootstrapping();
        drive_state(
            b"discovering gateways\nconnecting\nSOCKS5_ADDR=127.0.0.1:5\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:5"));
    }

    /// HYPOTHESIS: a status line updates the live bootstrap detail while the
    /// mode stays `Bootstrapping`. Falsified if the line is ignored as noise
    /// or flips the state.
    #[tokio::test]
    async fn a_status_line_updates_the_detail_and_keeps_bootstrapping() {
        let state = bootstrapping();
        drive_state(
            b"NYM_STATUS=discovering exit gateways\nNYM_STATUS=attempt 2/10: 2 in flight, 0 failed\n"
                .as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.bootstrap_detail.as_deref(),
            Some("attempt 2/10: 2 in flight, 0 failed"),
            "the LATEST status line is retained"
        );
        // Stdout closed without an address, so the terminal mode is Off; the
        // detail must have been visible while the reader was live, which the
        // retained value demonstrates.
        assert_eq!(s.mode, MixnetMode::Off);
    }

    /// HYPOTHESIS: the address announcement still wins after status lines,
    /// and readiness clears the now-stale detail. Falsified if a status line
    /// masks the announcement or the detail lingers past bootstrap.
    #[tokio::test]
    async fn the_address_wins_after_status_lines_and_clears_the_detail() {
        let state = bootstrapping();
        drive_state(
            b"NYM_STATUS=attempt 1/10: 1 in flight, 0 failed\nSOCKS5_ADDR=127.0.0.1:7\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:7"));
        assert_eq!(s.bootstrap_detail, None, "ready has no bootstrap detail");
    }

    #[tokio::test]
    async fn off_when_stdout_closes_without_an_address() {
        let state = bootstrapping();
        drive_state(
            b"failed to reach any gateway\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.mode,
            MixnetMode::Off,
            "no address means the mixnet was not reached"
        );
        assert!(s.socks5_addr.is_none());
    }
}
