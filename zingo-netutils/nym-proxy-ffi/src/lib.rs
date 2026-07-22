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

use std::sync::Mutex;

use tokio::runtime::Runtime;
use zingo_netutils::NymProxy;

uniffi::setup_scaffolding!();

/// Why starting or driving the mixnet proxy failed. Crosses the FFI as a
/// UniFFI error the host language can match on.
#[derive(Debug, thiserror::Error, uniffi::Error)]
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
}

/// The host implements this to learn that the proxy died after it was running,
/// so the app can redraw a fresh path and re-attach (the proxy-owner-remediates
/// contract). The shim invokes it at most once per proxy.
#[uniffi::export(callback_interface)]
pub trait ProxyDeathObserver: Send + Sync {
    /// Called once when the running proxy is lost, with a human-readable
    /// reason. The address previously reported is dead after this.
    fn on_death(&self, reason: String);
}

/// A running mixnet proxy the mobile host owns. Holds the tokio runtime that
/// keeps the [`NymProxy`] client and its SOCKS5 listener alive; dropping the
/// handle (or calling [`Self::stop`]) tears both down.
#[derive(uniffi::Object)]
pub struct MixnetProxyHandle {
    // The runtime must outlive the proxy: NymProxy's client runs background
    // tasks on it. Kept first so drop order tears the proxy down before the
    // runtime it ran on.
    runtime: Runtime,
    proxy: Mutex<Option<NymProxy>>,
    socks5_addr: String,
}

#[uniffi::export]
impl MixnetProxyHandle {
    /// Bring up a mixnet proxy and return a handle once its SOCKS5 listener is
    /// up. The returned handle's [`Self::socks5_address`] is what the app hands
    /// to the wallet's `attach_mixnet`; readiness is verified there (the
    /// increment-17 health round trip), so this returns as soon as the proxy
    /// has an address to offer.
    #[uniffi::constructor]
    pub fn start() -> Result<std::sync::Arc<Self>, ProxyFfiError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ProxyFfiError::Runtime {
                reason: e.to_string(),
            })?;
        let proxy = runtime
            .block_on(NymProxy::start())
            .map_err(|e| ProxyFfiError::Connect {
                reason: e.to_string(),
            })?;
        let socks5_addr = proxy.socks5_addr();
        Ok(std::sync::Arc::new(MixnetProxyHandle {
            runtime,
            proxy: Mutex::new(Some(proxy)),
            socks5_addr,
        }))
    }

    /// The local SOCKS5 address (`host:port`) the app hands to `attach_mixnet`.
    pub fn socks5_address(&self) -> String {
        self.socks5_addr.clone()
    }

    /// Disconnect the mixnet client and stop the local SOCKS5 proxy. Idempotent:
    /// a second call after the proxy is already stopped is a no-op.
    pub fn stop(&self) {
        let taken = self.proxy.lock().expect("proxy mutex poisoned").take();
        if let Some(proxy) = taken {
            self.runtime.block_on(proxy.disconnect());
        }
    }
}
