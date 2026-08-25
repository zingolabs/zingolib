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
/// cannot be retuned apart (issue #2565). Both gates run against a *cold*
/// tunnel — the first data through a fresh gateway session, where tail
/// latency lives — and issue #2564's field run showed healthy round trips
/// of ~3 s with tails that intermittently blew a 15-second bound, so the
/// bound is thirty seconds: roughly ten times the healthy round trip, and
/// still well inside [`NYM_LIFECYCLE_TIMEOUT`] even for the spawned
/// binary's three draws (3 × 30 s = 90 s of 120 s).
pub const MIXNET_ROUND_TRIP_BOUND: Duration = Duration::from_secs(30);

/// Bound on one loopback exchange with the local SOCKS5 listener, shared by
/// the wallet supervisor's watchdog and the mobile shim's monitor.
pub const LOOPBACK_DIAL_BOUND: Duration = Duration::from_secs(5);

/// Overall timeout for the mixnet bootstrap (`start()` and `reconnect()`),
/// preventing infinite hangs.
///
/// Nym SDK connection attempts can block indefinitely if a gateway is
/// unresponsive. This timeout caps total wall-clock time for the entire
/// retry loop. [`PER_ATTEMPT_CONNECT_TIMEOUT`] caps individual attempts.
pub const NYM_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for a single Exit Node connect attempt.
///
/// Without this bound, one unresponsive Exit Node hangs
/// `connect_to_mixnet_via_socks5` until the whole [`NYM_LIFECYCLE_TIMEOUT`]
/// budget burns, and the retry engine never reaches the next Exit Node. A
/// responsive Exit Node bootstraps in well under ten seconds. Six full
/// attempts fit inside the lifecycle budget.
pub const PER_ATTEMPT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Timeout for the Exit-Node-discovery API query, which is otherwise
/// unbounded for the same reason as the connect attempts.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the hedged bootstrap stays quiet before launching another
/// Exit Node pull in parallel. A responsive Exit Node typically connects in
/// well under ten seconds, so an attempt this old is worth hedging against
/// without yet giving up on it.
pub const HEDGE_INTERVAL: Duration = Duration::from_secs(5);

/// The mean exit-announcement latency the `birth-trial` workbench tool
/// measured over thirty pinned births against mainnet on 2026-08-18.
pub const OBSERVED_ANNOUNCEMENT_MEAN: Duration = Duration::from_millis(4_637);

/// The standard deviation of that same measurement, whose samples spanned
/// 3203 to 5604 milliseconds.
pub const OBSERVED_ANNOUNCEMENT_DEVIATION: Duration = Duration::from_millis(549);

/// How many standard deviations above the measured mean the readiness gate
/// waits before it refuses a transport that never announced.
pub const ANNOUNCEMENT_DEVIATIONS: u32 = 4;

/// How long the readiness gate waits for the transport's first Exit Node
/// announcement once the address has arrived: the four-deviation figure of
/// 6833 milliseconds rounded up to a whole second, so an unremarkably slow
/// bootstrap is waited through while one that never binds an exit is
/// refused long before the lifecycle budget.
///
/// ```
/// use zingo_netutils::time::{
///     ANNOUNCEMENT_DEVIATIONS, EXIT_ANNOUNCEMENT_GRACE, NYM_LIFECYCLE_TIMEOUT,
///     OBSERVED_ANNOUNCEMENT_DEVIATION, OBSERVED_ANNOUNCEMENT_MEAN,
/// };
///
/// // The grace covers four deviations above the measured mean, and the
/// // rounding that reaches a whole second never reaches a fifth.
/// let four = OBSERVED_ANNOUNCEMENT_MEAN + OBSERVED_ANNOUNCEMENT_DEVIATION * ANNOUNCEMENT_DEVIATIONS;
/// let five = OBSERVED_ANNOUNCEMENT_MEAN
///     + OBSERVED_ANNOUNCEMENT_DEVIATION * (ANNOUNCEMENT_DEVIATIONS + 1);
/// assert!(EXIT_ANNOUNCEMENT_GRACE >= four);
/// assert!(EXIT_ANNOUNCEMENT_GRACE < five);
/// assert!(EXIT_ANNOUNCEMENT_GRACE < NYM_LIFECYCLE_TIMEOUT);
/// ```
pub const EXIT_ANNOUNCEMENT_GRACE: Duration = Duration::from_millis(7_000);

/// The silence interval before a send's escalation launches a further
/// Destination arm: the sum of a connect attempt's bound and one mixnet
/// round trip, so a responsive Destination's confirmed delivery beats the
/// first hedge by construction, and the interval retunes when either bound
/// retunes.
///
/// ```
/// use zingo_netutils::time::{
///     MIXNET_ROUND_TRIP_BOUND, PER_ATTEMPT_CONNECT_TIMEOUT, TRANSMISSION_HEDGE_INTERVAL,
/// };
///
/// assert_eq!(
///     TRANSMISSION_HEDGE_INTERVAL,
///     PER_ATTEMPT_CONNECT_TIMEOUT + MIXNET_ROUND_TRIP_BOUND,
/// );
/// ```
pub const TRANSMISSION_HEDGE_INTERVAL: Duration =
    Duration::from_secs(PER_ATTEMPT_CONNECT_TIMEOUT.as_secs() + MIXNET_ROUND_TRIP_BOUND.as_secs());

/// How often the mobile shim's monitor dials the local SOCKS5 listener,
/// faster than the wallet's backstop so the app notices first.
pub const LISTENER_MONITOR_INTERVAL: Duration = Duration::from_secs(15);

/// Cadence of the wallet supervisor's loopback watchdog against an attached
/// endpoint.
pub const ATTACH_WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);

/// Pause between the attach readiness gate's round-trip attempts.
pub const ATTACH_LISTENER_RETRY_PAUSE: Duration = Duration::from_secs(1);

/// The attach readiness gate's total worst-case budget: every round-trip
/// attempt's bound plus the pauses between attempts (two attempts of
/// [`MIXNET_ROUND_TRIP_BOUND`] with one [`ATTACH_LISTENER_RETRY_PAUSE`]).
pub const ATTACH_READINESS_BUDGET: Duration = Duration::from_secs(
    MIXNET_ROUND_TRIP_BOUND.as_secs() * 2 + ATTACH_LISTENER_RETRY_PAUSE.as_secs(),
);

// ---------------------------------------------------------------------------
// The gRPC data path (sync and send)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// HYPOTHESIS: one bounded round trip fits inside the nym lifecycle
    /// budget.
    #[test]
    fn the_round_trip_bound_fits_inside_the_lifecycle() {
        assert!(
            MIXNET_ROUND_TRIP_BOUND <= NYM_LIFECYCLE_TIMEOUT,
            "retune the round-trip bound with the lifecycle, never apart"
        );
    }
}

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

/// How long a transmitting command waits out a bootstrapping mixnet
/// before the typed Bootstrapping refusal stands.
pub const TRANSMIT_READINESS_BUDGET: Duration = Duration::from_secs(90);

/// The cadence at which a waiting transmitting command reports that the
/// mixnet is still bootstrapping.
pub const TRANSMIT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);

/// Bound on one migration-part submission through the tunnel. More
/// patient than [`DEFAULT_REQUEST_TIMEOUT`] because a part transmission
/// tolerates latency better than an interactive send.
pub const MIGRATION_SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on a superseded conduit's wait for work already dialed through
/// it, set to the longest bounded operation that work can be.
pub const CONDUIT_DRAIN_BUDGET: Duration = MIGRATION_SUBMIT_TIMEOUT;

/// How often a superseded conduit is rechecked for idleness, the overlap a
/// rotation pays past its predecessor's last use.
pub const CONDUIT_DRAIN_POLL: Duration = Duration::from_millis(250);

/// How long to wait between sync polls while a note-splitting migration
/// round confirms.
pub const CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Every dispatched CLI command narrates its latest progress line at this
/// interval while it runs, so no command is silent past one interval and a
/// command that completes before the first tick stays silent.
pub const PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Diagnostics and server selection
// ---------------------------------------------------------------------------

/// How long each paired-probe leg may take. Generous for the mixnet leg's
/// tunnel establishment. A hanging exit is reported as a timeout, not
/// waited out.
pub const PROBE_LEG_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the Sentinel may take before its silence indicts the exit.
/// Shorter than a probe leg because the Sentinel's address is reliable
/// enough that silence is evidence about the tunnel, and measured round
/// trips through a live exit landed under two seconds.
pub const SENTINEL_BUDGET: Duration = Duration::from_millis(3_500);

/// How many expected proof cycles a whole speed-prioritized acquisition —
/// every redraw, birth, and wave together — may spend (ruled 2026-08-14).
pub const SPEED_ACQUISITION_PROOFS: u64 = 10;

/// ```
/// // One shared deadline bounds a speed-prioritized operation end to end:
/// // ten expected proof cycles, each an unremarkable bootstrap's exit
/// // announcement plus the Sentinel exchange (105 seconds, ruled
/// // 2026-08-14 and retuned when the grace was measured).
/// use zingo_netutils::time::{
///     EXIT_ANNOUNCEMENT_GRACE, SENTINEL_BUDGET, SPEED_ACQUISITION_DEADLINE,
///     SPEED_ACQUISITION_PROOFS,
/// };
/// assert_eq!(
///     SPEED_ACQUISITION_DEADLINE.as_millis(),
///     (EXIT_ANNOUNCEMENT_GRACE.as_millis() + SENTINEL_BUDGET.as_millis())
///         * SPEED_ACQUISITION_PROOFS as u128
/// );
/// assert_eq!(SPEED_ACQUISITION_DEADLINE.as_secs(), 105);
/// ```
pub const SPEED_ACQUISITION_DEADLINE: Duration = Duration::from_millis(
    (EXIT_ANNOUNCEMENT_GRACE.as_millis() as u64 + SENTINEL_BUDGET.as_millis() as u64)
        * SPEED_ACQUISITION_PROOFS,
);

/// ```
/// // One Nym network epoch: the hourly topology rotation after which an
/// // observation about an Exit Node describes a network that no longer
/// // exists.
/// use zingo_netutils::time::NYM_EPOCH;
/// assert_eq!(NYM_EPOCH, std::time::Duration::from_secs(60 * 60));
/// ```
// TODO: implement sensitivity to, and policy around, real Nym epoch
// boundaries: the live epoch's bounds are queryable from the same API the
// exit discovery uses, and this constant approximates the rotation cadence
// as a sliding window.
pub const NYM_EPOCH: Duration = Duration::from_secs(60 * 60);

/// The shortest a session holds one mixnet client before rotating it, on a
/// platform whose policy asks for rotation at all (ADR 0048).
pub const CLIENT_ROTATION_MIN: Duration = Duration::from_secs(5 * 60);

/// The longest, so no exit observes more than this much of one session.
///
/// ```
/// use zingo_netutils::time::{CLIENT_ROTATION_MAX, CLIENT_ROTATION_MIN, NYM_EPOCH};
///
/// assert!(CLIENT_ROTATION_MIN < CLIENT_ROTATION_MAX);
/// // A rotation bounds exposure more tightly than an epoch does, which is
/// // the whole point of rotating rather than waiting for the topology.
/// assert!(CLIENT_ROTATION_MAX < NYM_EPOCH);
/// ```
pub const CLIENT_ROTATION_MAX: Duration = Duration::from_secs(10 * 60);

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

    /// Cadence of the FFI listener monitor under paused-time tests.
    pub const MONITOR_CHECK_INTERVAL: Duration = Duration::from_millis(30);

    /// Per-check bound of the FFI listener monitor under paused-time tests.
    pub const MONITOR_CHECK_TIMEOUT: Duration = Duration::from_millis(500);
}
