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

/// The IP-correlation disclaimer a frontend must show alongside Mixnet Mode
/// status, satisfying ZIP-0318's requirement that the wallet explain the
/// IP-correlation risk.
///
/// Mixnet Mode obfuscates only the Transmission and price-fetch surfaces;
/// synchronization stays on the ordinary connector (ADR 0011, "Per-surface
/// transport tiers"). A bare "ready" status would let a user believe sync is
/// protected too, so this text names the residual exposure: the sync indexer
/// still learns the client IP and can correlate it with the wallet's on-chain
/// activity, and a reused IP can leak the wallet's total balance. It is kept
/// here as one canonical string so every frontend renders the same disclaimer
/// rather than each paraphrasing the risk.
pub const IP_CORRELATION_DISCLAIMER: &str = "\
IP-correlation risk: Mixnet Mode covers only transaction broadcast and \
price-fetch. Wallet synchronization always uses the ordinary connection, so \
the sync indexer — and any network operator on that path — sees your IP \
address and can correlate it with the transactions you broadcast; reusing the \
same IP across sessions can reveal your wallet's total balance to that \
operator. To hide your IP during synchronization as well, route the wallet \
through a system-level VPN or NymVPN. See ZIP-0318.";
