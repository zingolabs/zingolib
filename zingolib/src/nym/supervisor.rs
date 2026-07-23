//! The Mixnet Mode proxy supervisor (ADR 0011, consumption model A).
//!
//! The wallet cannot link the mixnet transport in-process, so it bundles the
//! `nym-proxy` binary and spawns it as a child. This supervisor owns that
//! child's lifecycle: it starts the process, reads the local SOCKS5 address
//! the child announces on stdout, and drives the tri-state
//! [`MixnetMode`](crate::nym::MixnetMode). While the child is starting the
//! mode is `Bootstrapping`; it becomes `Ready` once the address arrives. If
//! the child's stdout later closes — during bootstrap or after ready — the
//! mode becomes `Died`, an unconsented loss of the transport that makes
//! mixnet-only surfaces fail closed rather than fall back to clearnet. Only a
//! deliberate [`Self::stop`] yields `Off`.
//!
//! # Lifetime coupling
//!
//! The supervisor and its child behave as one unit (ADR 0011). Two mechanisms
//! bind the child's lifetime to the parent's without letting a terminal signal
//! tear the transport out from under a live session:
//!
//! - The child is spawned in its **own process group**, so a terminal `Ctrl-C`
//!   — delivered to the shell's foreground group — does not reach it. `Ctrl-C`
//!   aborts the wallet's current command while the mixnet keeps running.
//! - The supervisor holds the child's **stdin pipe** open for the child's
//!   life. The child watches that pipe; any parent exit — clean, panic, or
//!   `SIGKILL` (which skips `kill_on_drop`) — closes the pipe, the child reads
//!   EOF, disconnects from the mixnet, and exits. No orphaned proxy survives a
//!   dead parent.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, ChildStdin, Command};
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
    /// The spawned child exposed no stdin to hold open as the liveness pipe.
    #[error("the nym-proxy child exposed no stdin")]
    NoStdin,
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

/// Supervises the spawned `nym-proxy` child process and exposes its state.
pub struct MixnetProxy {
    child: Child,
    state: Arc<Mutex<ProxyState>>,
    reader: JoinHandle<()>,
    /// The child's stdin, held open for the child's life. The child watches
    /// this pipe and exits when it closes, so dropping this handle — on
    /// [`Self::stop`] or when the whole process dies — tears the proxy down.
    /// Never written to; its openness is the signal.
    _stdin: ChildStdin,
}

impl MixnetProxy {
    /// Spawn the `nym-proxy` binary at `binary_path`. Returns immediately with
    /// mode [`MixnetMode::Bootstrapping`]; poll [`Self::mode`] for readiness.
    /// The child is killed if this supervisor is dropped, spawned in its own
    /// process group (terminal signals do not reach it) with its stdin piped
    /// (its closure is how the child learns the parent is gone).
    pub fn spawn(binary_path: &Path) -> Result<Self, MixnetProxyError> {
        let mut command = Command::new(binary_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        // A distinct process group detaches the child from the terminal's
        // foreground group, so a user's Ctrl-C aborts the wallet command
        // without killing the privacy transport. Unix-only; on other targets
        // the child shares the group and Ctrl-C still reaches it (the
        // stdin-EOF watchdog remains the durable coupling).
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|source| MixnetProxyError::Spawn {
            path: binary_path.display().to_string(),
            source,
        })?;
        let stdout = child.stdout.take().ok_or(MixnetProxyError::NoStdout)?;
        let stdin = child.stdin.take().ok_or(MixnetProxyError::NoStdin)?;
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
            _stdin: stdin,
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

/// Read `stdout` for the child's whole life: the address announcement flips
/// the mode to `Ready`, progress lines update the live bootstrap detail, and —
/// the key coupling change — reading continues *past* `Ready` so a later close
/// is observed. When stdout closes at all, whether before or after the address
/// arrived, the mode becomes `Died`: an unexpected loss of the proxy, not a
/// consented `Off`. Only [`MixnetProxy::stop`] sets `Off`, and it aborts this
/// task first so its deliberate `Off` is never overwritten by this `Died`.
/// Generic over the reader so the state machine is unit-tested without a
/// process.
async fn drive_state<R: AsyncRead + Unpin>(stdout: R, state: Arc<Mutex<ProxyState>>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(addr) = parse_socks5_addr_line(&line) {
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.socks5_addr = Some(addr.to_string());
            guarded.bootstrap_detail = None;
            guarded.mode = MixnetMode::Ready;
            // Keep reading: a close after this must be observed as Died.
            continue;
        }
        if let Some(detail) = parse_status_line(&line) {
            state.lock().expect("proxy state mutex").bootstrap_detail = Some(detail.to_string());
        }
    }
    // Stdout closed. The child exited without a deliberate stop(), so the
    // transport is lost: refuse, never leak to clearnet. A stale address is
    // cleared so no surface can dial a dead proxy.
    let mut guarded = state.lock().expect("proxy state mutex");
    guarded.mode = MixnetMode::Died;
    guarded.socks5_addr = None;
    guarded.bootstrap_detail = None;
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
    use std::io::Cursor;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    fn bootstrapping() -> Arc<Mutex<ProxyState>> {
        Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: None,
        }))
    }

    /// A reader that yields `bytes` once and then stays pending forever, never
    /// reaching EOF — modelling a live child whose stdout is open but idle
    /// after its announcement. A plain byte slice EOFs immediately, which
    /// `drive_state` now (correctly) reads as the child dying; these tests
    /// want to observe the intermediate `Ready` while the stream is still
    /// open, so the reader must not close.
    struct OpenAfter(Cursor<Vec<u8>>);

    impl OpenAfter {
        fn new(bytes: &[u8]) -> Self {
            OpenAfter(Cursor::new(bytes.to_vec()))
        }
    }

    impl AsyncRead for OpenAfter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let position = self.0.position() as usize;
            let source = self.0.get_ref();
            if position >= source.len() {
                // Delivered everything; stay open, never signal EOF.
                return Poll::Pending;
            }
            let count = (source.len() - position).min(buf.remaining());
            buf.put_slice(&source[position..position + count]);
            self.0.set_position((position + count) as u64);
            Poll::Ready(Ok(()))
        }
    }

    /// Runs `drive_state` against an open-then-idle stream carrying `bytes`,
    /// yields until the mode leaves `Bootstrapping` (or a bound elapses), and
    /// returns the observed state without the stream ever closing.
    async fn state_over_open_stream(bytes: &[u8]) -> ProxyState {
        let state = bootstrapping();
        let handle = tokio::spawn(drive_state(OpenAfter::new(bytes), Arc::clone(&state)));
        for _ in 0..1000 {
            if state.lock().unwrap().mode != MixnetMode::Bootstrapping {
                break;
            }
            tokio::task::yield_now().await;
        }
        handle.abort();
        let guard = state.lock().expect("proxy state mutex");
        ProxyState::clone(&guard)
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
        let s = state_over_open_stream(b"SOCKS5_ADDR=127.0.0.1:43210\n").await;
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:43210"));
    }

    #[tokio::test]
    async fn ready_after_preamble_lines() {
        let s =
            state_over_open_stream(b"discovering gateways\nconnecting\nSOCKS5_ADDR=127.0.0.1:5\n")
                .await;
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:5"));
    }

    /// HYPOTHESIS: a status line updates the live bootstrap detail while the
    /// mode stays `Bootstrapping`. Falsified if the line is ignored as noise
    /// or flips the state.
    #[tokio::test]
    async fn a_status_line_updates_the_detail_and_keeps_bootstrapping() {
        // No address arrives, so the mode stays Bootstrapping and the helper
        // returns after its bound with the live detail retained. This isolates
        // the detail-tracking from the close transition (tested separately).
        let s = state_over_open_stream(
            b"NYM_STATUS=discovering exit gateways\nNYM_STATUS=attempt 2/10: 2 in flight, 0 failed\n",
        )
        .await;
        assert_eq!(s.mode, MixnetMode::Bootstrapping);
        assert_eq!(
            s.bootstrap_detail.as_deref(),
            Some("attempt 2/10: 2 in flight, 0 failed"),
            "the LATEST status line is retained while bootstrapping"
        );
    }

    /// HYPOTHESIS: the address announcement still wins after status lines,
    /// and readiness clears the now-stale detail. Falsified if a status line
    /// masks the announcement or the detail lingers past bootstrap.
    #[tokio::test]
    async fn the_address_wins_after_status_lines_and_clears_the_detail() {
        let s = state_over_open_stream(
            b"NYM_STATUS=attempt 1/10: 1 in flight, 0 failed\nSOCKS5_ADDR=127.0.0.1:7\n",
        )
        .await;
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:7"));
        assert_eq!(s.bootstrap_detail, None, "ready has no bootstrap detail");
    }

    #[tokio::test]
    async fn died_when_stdout_closes_during_bootstrap() {
        let state = bootstrapping();
        drive_state(
            b"failed to reach any gateway\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.mode,
            MixnetMode::Died,
            "a proxy that closes without an address died; it is not consented Off"
        );
        assert!(s.socks5_addr.is_none());
    }

    /// HYPOTHESIS: the reader keeps watching past Ready, so a proxy that dies
    /// AFTER announcing its address lands in Died with the stale address
    /// cleared — the exact zombie path the coupling closes. Falsified if the
    /// reader stops at Ready (mode stuck Ready) or leaves the dead address
    /// dialable.
    #[tokio::test]
    async fn died_when_stdout_closes_after_ready() {
        let state = bootstrapping();
        // Address announced (Ready), then stdout closes (the child exited).
        drive_state(
            b"SOCKS5_ADDR=127.0.0.1:43210\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.mode,
            MixnetMode::Died,
            "a proxy that dies after ready must not stay stale-Ready"
        );
        assert!(
            s.socks5_addr.is_none(),
            "the dead proxy's address must be cleared so nothing dials it"
        );
    }
}
