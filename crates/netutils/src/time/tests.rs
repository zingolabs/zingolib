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
