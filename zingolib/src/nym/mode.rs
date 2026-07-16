//! The tri-state runtime state of the Nym mixnet (Mixnet Mode).
#![forbid(unsafe_code)]

/// The runtime state of the Nym mixnet transport that carries the
/// Transmission and price-fetch surfaces.
///
/// This is tri-state rather than a bare boolean because "enabled but not yet
/// reachable" is a state a user interface must distinguish: a send attempted
/// while [`MixnetMode::Bootstrapping`] must wait or report "connecting", and
/// must never silently fall back to clearnet. See
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
}

impl MixnetMode {
    /// Whether a mixnet-only surface may proceed over the mixnet right now.
    /// True only in [`MixnetMode::Ready`].
    pub fn is_ready(self) -> bool {
        matches!(self, MixnetMode::Ready)
    }
}
