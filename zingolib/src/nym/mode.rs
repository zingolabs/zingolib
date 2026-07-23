//! The tri-state runtime state of the Nym mixnet (Mixnet Mode).
#![forbid(unsafe_code)]

/// The runtime state of the Nym mixnet transport that carries the
/// Transmission and price-fetch surfaces.
///
/// This has four states rather than a bare boolean because "enabled but not
/// yet reachable" and "was running, then died" are states a user interface
/// must distinguish from a deliberate off: a send attempted while
/// [`MixnetMode::Bootstrapping`] or after [`MixnetMode::Died`] must refuse or
/// report, and must never silently fall back to clearnet. Only the user's
/// deliberate [`MixnetMode::Off`] consents to clearnet. See
/// `docs/adr/0011-nym-mixnet-transmission.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixnetMode {
    /// The mixnet is disabled. Mixnet-only surfaces route over clearnet, but
    /// only as the user's deliberate per-session choice, never as a silent
    /// fallback.
    Off,
    /// The mixnet client is starting up and is not yet reachable. Mixnet-only
    /// surfaces are unavailable but connecting.
    Bootstrapping,
    /// The mixnet is up. Mixnet-only surfaces route through it.
    Ready,
    /// The proxy exited unexpectedly after being spawned — during bootstrap or
    /// after reaching ready. Distinct from [`MixnetMode::Off`]: this is an
    /// unconsented loss of the transport, so mixnet-only surfaces refuse
    /// rather than fall back to clearnet. Recover by re-enabling the mixnet
    /// (`nym on`), which spawns a fresh proxy.
    Died,
}

impl MixnetMode {
    /// Whether a mixnet-only surface may proceed over the mixnet right now.
    /// True only in [`MixnetMode::Ready`].
    pub fn is_ready(self) -> bool {
        matches!(self, MixnetMode::Ready)
    }
}
