use super::*;

#[test]
fn empty_schedule_matches_the_bare_regtest_default() {
    let parsed = parse_regtest_activation_heights("").expect("empty schedule is valid");
    assert_eq!(parsed, ActivationHeights::default());
}

#[test]
fn fixture_schedule_parses_to_the_mixed_chain() {
    // The harness fixture shape: pre-NU5 upgrades at 1, NU5/NU6 at 2,
    // the NU6.x line at 5, NU7 unscheduled.
    let parsed =
        parse_regtest_activation_heights("all=1,nu5=2,nu6=2,nu6_1=5,nu6_2=5,nu6_3=5,nu7=off")
            .expect("the fixture schedule is valid");
    assert_eq!(parsed.canopy(), Some(1));
    assert_eq!(parsed.nu5(), Some(2));
    assert_eq!(parsed.nu6(), Some(2));
    assert_eq!(parsed.nu6_1(), Some(5));
    assert_eq!(parsed.nu6_2(), Some(5));
    assert_eq!(parsed.nu6_3(), Some(5));
    assert_eq!(parsed.nu7(), None);
}

#[test]
fn entries_apply_left_to_right_over_all() {
    let parsed = parse_regtest_activation_heights("all=3,nu6_3=7").expect("override is valid");
    assert_eq!(parsed.nu6(), Some(3));
    assert_eq!(parsed.nu6_3(), Some(7));
}

#[test]
fn unknown_upgrade_is_an_error_not_a_silent_drop() {
    let error = parse_regtest_activation_heights("nu99=1")
        .expect_err("an unknown upgrade must be rejected");
    assert!(error.contains("nu99"), "error names the bad key: {error}");
}

#[test]
fn malformed_height_is_an_error() {
    let error = parse_regtest_activation_heights("nu5=soon")
        .expect_err("a non-numeric height must be rejected");
    assert!(error.contains("soon"), "error names the bad value: {error}");
}

#[test]
fn hint_without_schedule_prefix_still_fails_init() {
    let error = match build_connection_params(IndexerConnection {
        indexer: IndexerChoice::Offline,
        chain_hint: "shmegtest".to_string(),
        sync: SyncSettings {
            performance_level: "Medium".to_string(),
            min_confirmations: 1,
        },
    }) {
        Err(error) => error,
        Ok(_) => panic!("an unknown chain hint must be rejected"),
    };
    assert!(matches!(error, ZingolibError::Init(_)));
}
