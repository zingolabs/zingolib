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
use zingo_netutils::SOCKS5_ADDR_LINE_PREFIX;

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

    /// Shut the child down and stop tracking its state.
    pub async fn stop(mut self) {
        self.reader.abort();
        let _ = self.child.kill().await;
        self.state.lock().expect("proxy state mutex").mode = MixnetMode::Off;
    }
}

/// Read `stdout` until the child announces its SOCKS5 address (then `Ready`),
/// or until stdout closes without one (then `Off`). Generic over the reader so
/// the state machine is unit-tested without spawning a process.
async fn drive_state<R: AsyncRead + Unpin>(stdout: R, state: Arc<Mutex<ProxyState>>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(addr) = parse_socks5_addr_line(&line) {
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.socks5_addr = Some(addr.to_string());
            guarded.mode = MixnetMode::Ready;
            return;
        }
    }
    state.lock().expect("proxy state mutex").mode = MixnetMode::Off;
}

/// Extract the SOCKS5 address from a child stdout line, if it is the
/// announcement line.
fn parse_socks5_addr_line(line: &str) -> Option<&str> {
    line.strip_prefix(SOCKS5_ADDR_LINE_PREFIX).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bootstrapping() -> Arc<Mutex<ProxyState>> {
        Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
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
