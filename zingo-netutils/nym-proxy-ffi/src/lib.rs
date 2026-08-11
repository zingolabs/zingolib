//! The Nym mixnet proxy as a UniFFI component for mobile (ADR 0011 mobile
//! amendment, CP-3).
//!
//! iOS cannot spawn the `nym-proxy` child process the desktop model relies on,
//! and nym-sdk cannot link into the wallet's compile unit (the crypto-common
//! conflict). So on mobile the proxy is a dynamic library the app hosts: it
//! brings up a [`NymProxy`], exposes its local SOCKS5 address, and the app
//! hands that address to the wallet's `attach_mixnet` seam (CP-2). This crate
//! is that library's UniFFI surface — `start`, `stop`, and a death callback —
//! built from the standalone netutils workspace so nym-sdk resolves in its own
//! lockfile.
//!
//! The boundary is UniFFI rather than a hand-written C ABI specifically to
//! preserve `#![forbid(unsafe_code)]`: uniffi's proc-macros generate the FFI
//! `unsafe` scaffolding with macro hygiene that the lint does not fire on,
//! while any hand-written `unsafe` in this crate would still be rejected. So
//! this shim carries zero hand-written unsafe and keeps the workspace-wide
//! safety invariant intact — see the ADR amendment for the empirical check.
#![forbid(unsafe_code)]

use std::{net::SocketAddr, sync::Mutex, time::Duration};

use tokio::runtime::Runtime;
use zingo_netutils::responsiveness::PrioritiseSpeed;
use zingo_netutils::time::{LISTENER_MONITOR_INTERVAL, LOOPBACK_DIAL_BOUND};
use zingo_netutils::NymProxy;

#[cfg(target_os = "android")]
use zingo_nym_tls_init as _;

/// Consecutive check failures required before the proxy is declared dead.
const LISTENER_MONITOR_STRIKES: u32 = 2;

uniffi::setup_scaffolding!();

/// The local SOCKS5 endpoint a running proxy listens on, as explicit parts
/// rather than a formatted `host:port` string, so the host language gets a
/// typed port and never re-parses.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Socks5Endpoint {
    /// The listener's IP address literal, e.g. `"127.0.0.1"`.
    pub host: String,
    /// The listener's TCP port.
    pub port: u16,
}

/// Why starting or driving the mixnet proxy failed, crossing the FFI as a
/// UniFFI error the host language can match on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum ProxyFfiError {
    /// The tokio runtime that hosts the proxy could not be built.
    #[error("could not start the proxy runtime: {reason}")]
    Runtime {
        /// The underlying runtime-construction failure.
        reason: String,
    },
    /// The Nym mixnet client failed to connect.
    #[error("could not connect the mixnet proxy: {reason}")]
    Connect {
        /// The underlying NymProxy failure.
        reason: String,
    },
    /// The proxy came up but reported a listener address that did not parse
    /// as a socket address, so no endpoint can be offered.
    #[error("the proxy reported an unparseable SOCKS5 address: {reason}")]
    Address {
        /// The unparseable address text and the parse failure.
        reason: String,
    },
}

/// Why a running proxy was lost, as a typed cause the host can match on for
/// policy (retry, narrate, escalate), each variant carrying the underlying
/// diagnostic for logs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ProxyDeathReason {
    /// The mixnet client's connection to its gateway ended.
    MixnetDisconnected {
        /// The underlying disconnect diagnostic.
        detail: String,
    },
}

/// The host implements this to learn — at most once per proxy — that the
/// proxy died after it was running, so the app can redraw a fresh path and
/// re-attach under the proxy-owner-remediates contract.
#[uniffi::export(callback_interface)]
pub trait ProxyDeathObserver: Send + Sync {
    /// Called at most once when the running proxy is lost, after which the
    /// previously reported endpoint is dead.
    fn on_death(&self, reason: ProxyDeathReason);
}

/// A running mixnet proxy the mobile host owns, holding the tokio runtime
/// that keeps the [`NymProxy`] client and its SOCKS5 listener alive until
/// [`Self::stop`] or a drop of the handle tears both down.
#[derive(uniffi::Object)]
pub struct MixnetProxyHandle {
    // The runtime must outlive the proxy: NymProxy's client runs background
    // tasks on it. Kept first so drop order tears the proxy down before the
    // runtime it ran on.
    runtime: Runtime,
    proxy: Mutex<Option<NymProxy>>,
    endpoint: Socks5Endpoint,
    monitor: Option<tokio::task::AbortHandle>,
}

/// Why one SOCKS5 method-selection round trip failed.
#[derive(Debug, thiserror::Error)]
enum HandshakeCheckError {
    /// The TCP connect to the listener failed.
    #[error("connect: {0}")]
    Connect(std::io::Error),
    /// The greeting write failed.
    #[error("greeting write: {0}")]
    GreetingWrite(std::io::Error),
    /// The method-selection read failed.
    #[error("method-selection read: {0}")]
    MethodSelectionRead(std::io::Error),
    /// The listener answered a method selection other than no-auth.
    #[error("unexpected method selection {reply:?}")]
    MethodSelection {
        /// The two reply bytes the listener answered.
        reply: [u8; 2],
    },
    /// The whole round trip missed its budget.
    #[error("check timed out after {}ms", budget.as_millis())]
    TimedOut {
        /// The budget the round trip missed.
        budget: Duration,
    },
}

/// One purely local SOCKS5 method-selection round trip against the listener,
/// answered `[0x05, 0x00]` when the no-auth method is accepted.
async fn socks5_handshake_check(
    endpoint: &Socks5Endpoint,
    check_timeout: Duration,
) -> Result<(), HandshakeCheckError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(HandshakeCheckError::Connect)?;
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(HandshakeCheckError::GreetingWrite)?;
        let mut reply = [0u8; 2];
        stream
            .read_exact(&mut reply)
            .await
            .map_err(HandshakeCheckError::MethodSelectionRead)?;
        if reply != [0x05, 0x00] {
            return Err(HandshakeCheckError::MethodSelection { reply });
        }
        Ok(())
    };
    tokio::time::timeout(check_timeout, attempt)
        .await
        .map_err(|_| HandshakeCheckError::TimedOut {
            budget: check_timeout,
        })?
}

/// Checks the listener until `strikes_allowed` consecutive failures, then
/// reports the death exactly once and ends.
async fn monitor_listener(
    endpoint: Socks5Endpoint,
    observer: Box<dyn ProxyDeathObserver>,
    interval: Duration,
    check_timeout: Duration,
    strikes_allowed: u32,
) {
    let mut strikes = 0;
    loop {
        tokio::time::sleep(interval).await;
        match socks5_handshake_check(&endpoint, check_timeout).await {
            Ok(()) => strikes = 0,
            Err(failure) => {
                strikes += 1;
                if strikes >= strikes_allowed {
                    // The typed failure is rendered only here, at the FFI
                    // schema's existing prose field.
                    observer.on_death(ProxyDeathReason::MixnetDisconnected {
                        detail: failure.to_string(),
                    });
                    return;
                }
            }
        }
    }
}

/// Split a `NymProxy` listener address (`"127.0.0.1:43210"`) into the typed
/// endpoint the FFI surface offers, kept pure so the derivation is
/// unit-testable against round trips through [`SocketAddr`].
fn endpoint_from_listener_addr(addr: &str) -> Result<Socks5Endpoint, ProxyFfiError> {
    let parsed: SocketAddr = addr.parse().map_err(|e| ProxyFfiError::Address {
        reason: format!("{addr:?}: {e}"),
    })?;
    Ok(Socks5Endpoint {
        host: parsed.ip().to_string(),
        port: parsed.port(),
    })
}

#[uniffi::export]
impl MixnetProxyHandle {
    /// Bring up a mixnet proxy and return, once its SOCKS5 listener is up, a
    /// handle whose [`Self::socks5_endpoint`] the app hands to the wallet's
    /// `attach_mixnet`, with a listener monitor that reports through
    /// `observer`, at most once, if the proxy is lost.
    #[uniffi::constructor]
    pub fn start(
        observer: Option<Box<dyn ProxyDeathObserver>>,
    ) -> Result<std::sync::Arc<Self>, ProxyFfiError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ProxyFfiError::Runtime {
                reason: e.to_string(),
            })?;
        let proxy = runtime
            .block_on(NymProxy::start::<PrioritiseSpeed>())
            .map_err(|e| ProxyFfiError::Connect {
                reason: e.to_string(),
            })?;
        let endpoint = endpoint_from_listener_addr(&proxy.socks5_addr())?;
        let monitor = observer.map(|observer| {
            runtime
                .spawn(monitor_listener(
                    endpoint.clone(),
                    observer,
                    LISTENER_MONITOR_INTERVAL,
                    LOOPBACK_DIAL_BOUND,
                    LISTENER_MONITOR_STRIKES,
                ))
                .abort_handle()
        });
        Ok(std::sync::Arc::new(MixnetProxyHandle {
            runtime,
            proxy: Mutex::new(Some(proxy)),
            endpoint,
            monitor,
        }))
    }

    /// The local SOCKS5 endpoint the app hands to `attach_mixnet`.
    pub fn socks5_endpoint(&self) -> Socks5Endpoint {
        self.endpoint.clone()
    }

    /// The Exit Node identity the running proxy bound, `None` once stopped.
    pub fn exit_node(&self) -> Option<String> {
        self.proxy
            .lock()
            .expect("proxy mutex poisoned")
            .as_ref()
            .map(|proxy| proxy.exit_node().to_string())
    }

    /// Disconnect the mixnet client and stop the local SOCKS5 proxy,
    /// idempotently, cancelling the liveness monitor before the listener goes
    /// down so a deliberate stop never fires the death observer.
    pub fn stop(&self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
        let taken = self.proxy.lock().expect("proxy mutex poisoned").take();
        if let Some(proxy) = taken {
            self.runtime.block_on(proxy.disconnect());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn endpoint_round_trips_the_listener_address_nym_proxy_binds() {
        // NymProxy always binds IPv4 loopback; the derivation must preserve
        // both parts exactly through SocketAddr and back.
        let bound = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43210);
        let endpoint = endpoint_from_listener_addr(&bound.to_string()).unwrap();
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 43210);
        let rebuilt: SocketAddr = format!("{}:{}", endpoint.host, endpoint.port)
            .parse()
            .unwrap();
        assert_eq!(rebuilt, bound);
    }

    #[test]
    fn unparseable_listener_address_is_a_typed_address_error() {
        let error = endpoint_from_listener_addr("not-an-address").unwrap_err();
        assert!(matches!(error, ProxyFfiError::Address { .. }));
    }

    use std::sync::Arc;

    /// A local stand-in for the nym client's SOCKS5 listener: answers each
    /// greeting with the given method-selection reply until dropped.
    async fn mock_socks5_listener(reply: [u8; 2]) -> (Socks5Endpoint, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let endpoint = endpoint_from_listener_addr(&listener.local_addr().unwrap().to_string())
            .expect("mock listener address parses");
        let serving = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut greeting = [0u8; 3];
                if stream.read_exact(&mut greeting).await.is_ok() {
                    let _ = stream.write_all(&reply).await;
                }
            }
        });
        (endpoint, serving)
    }

    use zingo_netutils::time::test::MONITOR_CHECK_TIMEOUT;

    #[tokio::test]
    async fn check_passes_a_listener_that_completes_the_handshake() {
        let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
        socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
            .await
            .expect("a completed handshake must pass the check");
        serving.abort();
    }

    #[tokio::test]
    async fn check_fails_a_listener_that_rejects_the_method() {
        // 0xff is SOCKS5 for "no acceptable method" — the listener is alive
        // but no longer serving, which must read as dead.
        let (endpoint, serving) = mock_socks5_listener([0x05, 0xff]).await;
        socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
            .await
            .expect_err("a refusing listener must fail the check");
        serving.abort();
    }

    #[tokio::test]
    async fn check_fails_when_nothing_listens() {
        let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
        serving.abort();
        // The port is now free again; connecting must be refused.
        tokio::time::sleep(Duration::from_millis(50)).await;
        socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
            .await
            .expect_err("a dead listener must fail the check");
    }

    /// HYPOTHESIS: a refusing listener is reported as the typed
    /// method-selection variant carrying the reply bytes, not as prose.
    /// Falsified if the variant or its payload differs.
    #[tokio::test]
    async fn check_names_the_refused_method_selection() {
        let (endpoint, serving) = mock_socks5_listener([0x05, 0xff]).await;
        let failure = socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
            .await
            .expect_err("a refusing listener must fail the check");
        assert!(
            matches!(
                failure,
                super::HandshakeCheckError::MethodSelection {
                    reply: [0x05, 0xff]
                }
            ),
            "the reply bytes travel typed: {failure:?}"
        );
        serving.abort();
    }

    #[derive(Default)]
    struct RecordingObserver {
        deaths: Mutex<Vec<ProxyDeathReason>>,
    }

    impl ProxyDeathObserver for Arc<RecordingObserver> {
        fn on_death(&self, reason: ProxyDeathReason) {
            self.deaths.lock().unwrap().push(reason);
        }
    }

    use zingo_netutils::time::test::MONITOR_CHECK_INTERVAL;

    #[tokio::test]
    async fn monitor_reports_death_exactly_once_after_the_listener_dies() {
        let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
        let observer = Arc::new(RecordingObserver::default());
        let monitor = tokio::spawn(monitor_listener(
            endpoint,
            Box::new(Arc::clone(&observer)),
            MONITOR_CHECK_INTERVAL,
            MONITOR_CHECK_TIMEOUT,
            LISTENER_MONITOR_STRIKES,
        ));
        // Several healthy intervals pass without a death report.
        tokio::time::sleep(MONITOR_CHECK_INTERVAL * 4).await;
        assert!(observer.deaths.lock().unwrap().is_empty());

        serving.abort();
        // The monitor ends itself after reporting; that is the at-most-once
        // guarantee, and joining it proves the report happened.
        tokio::time::timeout(zingo_netutils::time::test::MOCK_OP_BOUND, monitor)
            .await
            .expect("monitor must report within the strike window")
            .expect("monitor task must end cleanly");
        let deaths = observer.deaths.lock().unwrap();
        assert_eq!(deaths.len(), 1, "death must be reported exactly once");
        assert!(matches!(
            deaths[0],
            ProxyDeathReason::MixnetDisconnected { .. }
        ));
    }
}
