//! The one acquisition shape every mixnet operation shares: a hedged race
//! over a clutch of Exit Node reservations, winning by binding, with proof
//! of the bound exit belonging to the layer above the SOCKS5 seam.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// ```
/// // The number of exit reservations an acquisition draws to attempt one
/// // connection.
/// use zingo_netutils::responsiveness::RESERVATION_CLUTCH_SIZE;
/// assert_eq!(RESERVATION_CLUTCH_SIZE, 4);
/// ```
pub const RESERVATION_CLUTCH_SIZE: usize = 4;

/// ```
/// // Every Acquisition Race runs under the one hedged launch policy,
/// // capped at the clutch size and paced by the hedge interval.
/// use zingo_netutils::arm_race::LaunchPolicy;
/// use zingo_netutils::responsiveness::{
///     RESERVATION_CLUTCH_SIZE, acquisition_launch_policy,
/// };
/// use zingo_netutils::time::HEDGE_INTERVAL;
///
/// match acquisition_launch_policy() {
///     LaunchPolicy::Hedged {
///         max_parallel,
///         hedge_interval,
///     } => {
///         assert_eq!(max_parallel, RESERVATION_CLUTCH_SIZE);
///         assert_eq!(hedge_interval, HEDGE_INTERVAL);
///     }
///     other => panic!("the one policy is hedged, got {other:?}"),
/// }
/// ```
pub fn acquisition_launch_policy() -> LaunchPolicy {
    LaunchPolicy::Hedged {
        max_parallel: RESERVATION_CLUTCH_SIZE,
        hedge_interval: HEDGE_INTERVAL,
    }
}
