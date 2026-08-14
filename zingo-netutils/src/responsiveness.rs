//! The one acquisition shape every mixnet operation shares: a hedged race
//! over a clutch of Exit Node reservations, winning by binding, with proof
//! of the bound exit belonging to the layer above the SOCKS5 seam.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// The number of exit reservations an acquisition draws to attempt one
/// connection.
pub const RESERVATION_CLUTCH_SIZE: usize = 4;

/// The launch policy every Acquisition Race runs under.
pub fn acquisition_launch_policy() -> LaunchPolicy {
    LaunchPolicy::Hedged {
        max_parallel: RESERVATION_CLUTCH_SIZE,
        hedge_interval: HEDGE_INTERVAL,
    }
}
