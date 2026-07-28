//! The temporal parameters of every crate that can see this one, in one
//! place.
//!
//! Every timeout, cadence, pause, and window that tunes a mechanism in a
//! crate depending on `zingo-netutils` lives here, so a retune moves one
//! number per physical fact and a reader surveys every knob on one page
//! (issue #2565). Each constant documents the physical quantity it names;
//! two call sites share a constant only when they bound the *same*
//! quantity, never because their numbers coincide — the census behind this
//! module adjudicated every coincidence. Values that exist only for tests
//! live in the `test` submodule, which `cfg(test)` exposes to this crate's
//! own tests and the `testutils` feature exposes to downstream test code.
//!
//! The one production exception dependency direction forces: `zingo-price`
//! sits below the wallet with no dependency on this crate, so
//! `zingo_price::REQUEST_TIMEOUT` (20 s) and `zingo_price::CONNECT_TIMEOUT`
//! (10 s) stay there, role-tuned to a decorative datum under the mobile
//! UI's 25-second watchdog.
#![forbid(unsafe_code)]

use std::time::Duration;

// ---------------------------------------------------------------------------
// The mixnet transport (ADR 0011 and its mobile amendment)
// ---------------------------------------------------------------------------

/// Bound on one data round trip through the mixnet tunnel: the health
/// round trip the spawnable `nym-proxy` binary gates readiness on, and the
/// identical round trip the wallet's attach readiness gate runs (ADR 0011's
/// mobile amendment). One physical quantity, defined once so the two gates
/// cannot be retuned apart (issue #2565). A dead path stalls the TLS
/// handshake, so this must fire well within [`NYM_LIFECYCLE_TIMEOUT`].
pub const MIXNET_ROUND_TRIP_BOUND: Duration = Duration::from_secs(15);

/// Bound on one loopback exchange with the local SOCKS5 listener: the wallet
/// supervisor's liveness probe (a bare TCP dial) and the mobile shim's
/// liveness monitor (a SOCKS5 method-selection round trip) both address the
/// same in-process listener, so they share one bound (issue #2565). Generous
/// for a loopback exchange — its job is to notice a torn-down host, not to
/// measure the mixnet, which no local exchange can see.
pub const LOOPBACK_DIAL_BOUND: Duration = Duration::from_secs(5);

/// Overall timeout for the mixnet bootstrap (`start()` and `reconnect()`),
/// preventing infinite hangs.
///
/// Nym SDK connection attempts can block indefinitely if a gateway is
/// unresponsive. This timeout caps total wall-clock time for the entire
/// retry loop. [`PER_ATTEMPT_CONNECT_TIMEOUT`] caps individual attempts.
pub const NYM_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for a single provider connect attempt.
///
/// Without this bound, one unresponsive provider hangs
/// `connect_to_mixnet_via_socks5` until the whole [`NYM_LIFECYCLE_TIMEOUT`]
/// budget burns, and the retry engine never reaches the next provider. A
/// responsive provider bootstraps in well under ten seconds. Six full
/// attempts fit inside the lifecycle budget.
pub const PER_ATTEMPT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Timeout for the provider-discovery API query, which is otherwise
/// unbounded for the same reason as the connect attempts.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the hedged bootstrap stays quiet before launching another
/// provider in parallel. A responsive provider typically connects in well
/// under ten seconds, so an attempt this old is worth hedging against
/// without yet giving up on it.
pub const HEDGE_INTERVAL: Duration = Duration::from_secs(5);

/// How often the mobile shim's liveness monitor probes the local SOCKS5
/// listener. Faster than [`ATTACH_PROBE_INTERVAL`] because the shim's host
/// (the app) is the remediation owner: it must notice a lost proxy and
/// re-attach before the wallet's backstop declares death. Whether that
/// ordering is a hard constraint is an open question on issue #2565.
pub const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(15);

/// Cadence of the wallet supervisor's liveness probe against an attached
/// endpoint — the backstop for hosts that pass no death observer.
pub const ATTACH_PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// Pause between the attach readiness gate's round-trip attempts, letting a
/// transient blip pass. Spacing, not a bound: the attempts themselves are
/// bounded by [`MIXNET_ROUND_TRIP_BOUND`].
pub const ATTACH_HEALTH_RETRY_PAUSE: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// The gRPC data path (sync and send)
// ---------------------------------------------------------------------------

/// Bound on one ordinary unary indexer request: the wallet's default
/// patience for a single gRPC call on the send and query paths.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on waiting for the next message on a gRPC stream, so a stalled
/// server ends the wait as a typed timeout rather than hanging the consumer.
/// Shared by pepper-sync's client and scanner, which formerly each carried
/// their own identical copy (issue #2565).
pub const STREAM_MSG_TIMEOUT: Duration = Duration::from_secs(15);

/// Bound on one light unary RPC issued by pepper-sync's fetcher.
pub const UNARY_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on one heavy unary RPC issued by pepper-sync's fetcher, whose
/// responses are large enough to deserve more patience than
/// [`UNARY_RPC_TIMEOUT`].
pub const HEAVY_UNARY_TIMEOUT: Duration = Duration::from_secs(20);

/// How long pepper-sync's scanner waits for its workers to wind down before
/// abandoning a clean shutdown.
pub const SCANNER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// The mempool drain's worst-case wait: the pre-c90f8d309 unconditional
/// sleep, demoted to a ceiling so a stream that never connects cannot hold
/// the session open.
pub const MEMPOOL_DRAIN_CEILING: Duration = Duration::from_secs(1);

/// One settle window after the mempool subscription, inside
/// [`MEMPOOL_DRAIN_CEILING`].
pub const MEMPOOL_DRAIN_SETTLE: Duration = Duration::from_millis(200);

/// Bound on waiting for the sync engine to acknowledge a start request.
pub const SYNC_START_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// The send pipeline and its narration
// ---------------------------------------------------------------------------

/// The interval between transmit retries and queued-verdict probes.
pub const TRANSMIT_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Bound on one migration-broadcast submission through the tunnel. More
/// patient than [`DEFAULT_REQUEST_TIMEOUT`] because a migration broadcast
/// tolerates latency better than an interactive send.
pub const MIGRATION_SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait between sync polls while a note-splitting migration
/// round confirms.
pub const CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Mixnet transmissions can wait for minutes (mixnet round trips, per-arm
/// retries, serially gated fan-out rounds, queued-verdict probes), so every
/// transmitting CLI command prints the transmission's latest progress line
/// at this interval while it waits. A send that completes before the first
/// tick stays silent.
pub const TRANSMIT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Diagnostics and server selection
// ---------------------------------------------------------------------------

/// How long each paired-probe leg may take. Generous for the mixnet leg's
/// tunnel establishment. A hanging exit is reported as a timeout, not
/// waited out.
pub const PROBE_LEG_TIMEOUT: Duration = Duration::from_secs(20);

/// Per-server bound on the ranking `get_info` sweep, deliberately tight so
/// one slow server cannot block the fastest-first ordering.
pub const SERVER_RANKING_TIMEOUT: Duration = Duration::from_secs(5);

/// Temporal parameters owned by tests. Gated so production code cannot
/// reference it: this crate's own unit tests see it via `cfg(test)`, and
/// downstream crates' test code opts in through the `testutils` feature
/// (declared in their dev-dependencies, so the gate never leaks into a
/// production build).
#[cfg(any(test, feature = "testutils"))]
pub mod test {
    use std::time::Duration;

    /// Generous bound for an operation against a local mock that should
    /// complete in milliseconds; expiry means the code under test hung.
    pub const MOCK_OP_BOUND: Duration = Duration::from_secs(5);

    /// Bound for the TLS-handshake tests' requests against their local
    /// listener.
    pub const LOCAL_TLS_TEST_BOUND: Duration = Duration::from_secs(10);

    /// Idle window after which a quiet chain is declared genuinely quiet:
    /// the sentinel tests sleep this long, re-query, and assert nothing
    /// moved.
    pub const IDLE_OBSERVATION_WINDOW: Duration = Duration::from_secs(5);

    /// Deadline for a log line to reach the log file on disk.
    pub const LOG_FLUSH_DEADLINE: Duration = Duration::from_secs(15);

    /// Poll cadence while an integration test waits for chain state to
    /// settle.
    pub const SETTLE_POLL_INTERVAL: Duration = Duration::from_secs(5);

    /// Per-stage bound for the hand-run live staged probe against a public
    /// indexer over clearnet.
    pub const LIVE_STAGE_BOUND: Duration = Duration::from_secs(15);

    /// Bound on the indexer ingesting a submitted transaction into its
    /// mempool view.
    pub const MEMPOOL_INGEST_BOUND: Duration = Duration::from_secs(10);

    /// Bound on one mempool stream request while polling for a transaction.
    pub const MEMPOOL_STREAM_BOUND: Duration = Duration::from_secs(5);

    /// Bound on the wallet record leaving Transmitted status after a sync.
    pub const WALLET_RECORD_LAG_BOUND: Duration = Duration::from_secs(15);

    /// The simulated duration of a transmission in the CLI heartbeat tests.
    pub const SIMULATED_TRANSMIT: Duration = Duration::from_secs(5);

    /// The hedge interval the pure racing planner's paused-time tests use;
    /// deliberately independent of the production [`super::HEDGE_INTERVAL`]
    /// so planner tests never retune with production.
    pub const PLANNER_HEDGE: Duration = Duration::from_secs(5);

    /// A stage bound short enough to prove staged-probe timeouts on paused
    /// time.
    pub const FAST_STAGE_BOUND: Duration = Duration::from_millis(800);

    /// Cadence of the FFI liveness monitor under paused-time tests.
    pub const MONITOR_PROBE_INTERVAL: Duration = Duration::from_millis(30);

    /// Per-probe bound of the FFI liveness monitor under paused-time tests.
    pub const MONITOR_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
}
