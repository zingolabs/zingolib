use super::*;

#[test]
fn a_deliberate_switch_off_is_its_own_string() {
    assert_eq!(
        mixnet_indicator_string(zingolib::mixnet::Indicator::SwitchedOff),
        "off"
    );
}

#[test]
fn an_unconsented_loss_and_a_never_attached_transport_both_fail_closed() {
    for indicator in [
        zingolib::mixnet::Indicator::Died,
        zingolib::mixnet::Indicator::Unattached,
    ] {
        assert_eq!(
            mixnet_indicator_string(indicator),
            "died",
            "{indicator:?} must fail closed rather than read as a resting mode"
        );
    }
}

#[test]
fn a_proven_transport_routes_exactly_as_ready() {
    for indicator in [
        zingolib::mixnet::Indicator::Ready,
        zingolib::mixnet::Indicator::PreviouslyProvenThisEpoch,
    ] {
        assert_eq!(mixnet_indicator_string(indicator), "ready");
    }
}
