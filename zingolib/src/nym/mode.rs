//! The five-state runtime state of the Nym mixnet (Mixnet Mode).
#![forbid(unsafe_code)]

use crate::nym::MixnetProxy;

/// The runtime state of the Nym mixnet transport that carries the
/// Transmission and price-fetch surfaces.
///
/// Five states rather than a bare boolean because refusal and consent must
/// never share a representation: "no transport is established", "enabled but
/// not yet reachable", and "was running, then died" are all states a send
/// must refuse in, and none of them is the user's deliberate
/// [`MixnetMode::SwitchedOff`], the one state that consents to clearnet. See
/// `docs/adr/0011-nym-mixnet-transmission.md` (amendment 2026-07-28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixnetMode {
    /// No mixnet transport is established and no consent to clearnet has
    /// been recorded. A present condition, not a history claim: this is the
    /// initial state, and equally the state after a failed enable or
    /// re-enable, even when a transport ran earlier in the session.
    /// Mixnet-only surfaces refuse — the absence of a transport is never
    /// consent.
    Unattached,
    /// The mixnet is disabled by the user's explicit act, and only by it.
    /// Mixnet-only surfaces route over clearnet as the user's deliberate
    /// per-session choice, never as a silent fallback.
    SwitchedOff,
    /// The mixnet client is starting up and is not yet reachable. Mixnet-only
    /// surfaces are unavailable but connecting.
    Bootstrapping,
    /// The mixnet is up. Mixnet-only surfaces route through it.
    Ready,
    /// The proxy exited unexpectedly after being spawned, during bootstrap or
    /// after reaching ready. Distinct from [`MixnetMode::SwitchedOff`]: this
    /// is an unconsented loss of the transport, so mixnet-only surfaces
    /// refuse rather than fall back to clearnet. Recover by re-enabling the
    /// mixnet (`nym on`), which spawns a fresh proxy.
    Died,
}

impl MixnetMode {
    /// Whether a mixnet-only surface may proceed over the mixnet right now.
    /// True only in [`MixnetMode::Ready`].
    pub fn is_ready(self) -> bool {
        matches!(self, MixnetMode::Ready)
    }
}

/// The wallet's mixnet transport slot: the explicit state [`MixnetMode`] is
/// read from. An enum rather than `Option<MixnetProxy>` because dropping the
/// handle on disable would erase the very bit that separates
/// [`MixnetMode::SwitchedOff`] (consent to clearnet) from
/// [`MixnetMode::Unattached`] (absence of a transport) — the flattening the
/// 2026-07-28 amendment of ADR 0011 retires.
// One slot lives per client and never in a collection, so the size skew
// between the unit states and the attached transport costs nothing; boxing
// would add only indirection.
#[allow(clippy::large_enum_variant)]
pub(crate) enum MixnetSlot {
    /// No transport and no consent recorded. The initial state, and the
    /// state a failed enable leaves behind.
    Unattached,
    /// The user's deliberate per-session disable. The one slot state that
    /// consents to clearnet.
    SwitchedOff,
    /// A spawned or attached transport, in whatever lifecycle state it
    /// reports (bootstrapping, ready, or died).
    Attached(MixnetProxy),
}

impl MixnetSlot {
    /// The Mixnet Mode this slot is in: the slot's own state when no
    /// transport is attached, otherwise the transport's lifecycle state.
    pub(crate) fn mode(&self) -> MixnetMode {
        match self {
            MixnetSlot::Unattached => MixnetMode::Unattached,
            MixnetSlot::SwitchedOff => MixnetMode::SwitchedOff,
            MixnetSlot::Attached(proxy) => proxy.mode(),
        }
    }

    /// The attached transport, when one is present.
    pub(crate) fn proxy(&self) -> Option<&MixnetProxy> {
        match self {
            MixnetSlot::Attached(proxy) => Some(proxy),
            MixnetSlot::Unattached | MixnetSlot::SwitchedOff => None,
        }
    }
}

/// The IP-correlation disclaimer a frontend must show alongside Mixnet Mode
/// status, satisfying ZIP-0318's requirement that the wallet explain the
/// IP-correlation risk.
///
/// Mixnet Mode obfuscates only the Transmission and price-fetch surfaces, and
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
the sync indexer (and any network operator on that path) sees your IP \
address and can correlate it with the transactions you broadcast; reusing the \
same IP across sessions can reveal your wallet's total balance to that \
operator. To hide your IP during synchronization as well, route the wallet \
through a system-level VPN or NymVPN. See ZIP-0318.";
