use zcash_protocol::value::Zatoshis;

use super::{SendAllCorrection, send_all_correction};

fn zats(value: u64) -> Zatoshis {
    Zatoshis::const_from_u64(value)
}

#[test]
fn a_shortfall_below_the_amount_retries_with_the_amount_reduced() {
    assert_eq!(
        send_all_correction(zats(125_000), zats(155_000), zats(150_000)),
        SendAllCorrection::Retry(zats(120_000))
    );
}

#[test]
fn a_shortfall_equal_to_the_amount_leaves_nothing_to_send() {
    assert_eq!(
        send_all_correction(zats(5_000), zats(155_000), zats(150_000)),
        SendAllCorrection::NothingToSend
    );
}

#[test]
fn a_shortfall_above_the_amount_leaves_nothing_to_send() {
    assert_eq!(
        send_all_correction(zats(25_000), zats(1_035_000), zats(1_000_000)),
        SendAllCorrection::NothingToSend
    );
}

#[test]
fn a_refusal_with_no_shortfall_is_unbalanced() {
    assert_eq!(
        send_all_correction(zats(76_000), zats(101_000), zats(160_000)),
        SendAllCorrection::Unbalanced
    );
    assert_eq!(
        send_all_correction(zats(76_000), zats(160_000), zats(160_000)),
        SendAllCorrection::Unbalanced
    );
}
