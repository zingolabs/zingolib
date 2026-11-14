use super::*;

/// Real lines from the 2026-07-15 container run: streamed FAIL and
/// TIMEOUT statuses, the end-of-run recap re-printing one of them, a
/// retry line, and a PASS line. Only final non-passing statuses
/// survive, once each.
#[test]
fn collects_and_deduplicates_terminal_statuses() {
    let log = "        PASS [  50.205s] ( 1/40) libtonode-tests::concrete mine_to_transparent\n\
             \x20       FAIL [ 289.674s] (31/40) libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation\n\
             \x20    TIMEOUT [ 600.017s] (40/40) libtonode-tests::migration anchorless_part_skips_without_sync\n\
             \x20   TRY 2 FAIL [  1.002s] ( 2/40) libtonode-tests::concrete retried_test\n\
             \x20       FAIL [ 289.674s] (31/40) libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation\n";
    assert_eq!(
        parse_failures(log),
        vec![
            (
                "FAIL".to_string(),
                "libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation"
                    .to_string()
            ),
            (
                "TIMEOUT".to_string(),
                "libtonode-tests::migration anchorless_part_skips_without_sync".to_string()
            ),
        ]
    );
}

#[test]
fn survives_ansi_and_missing_progress_marker() {
    let log = "\x1b[1m\x1b[31mFAIL\x1b[0m [ 17.210s] libtonode-tests::sync store_all_checkpoints_in_verification_window\n";
    assert_eq!(
        parse_failures(log),
        vec![(
            "FAIL".to_string(),
            "libtonode-tests::sync store_all_checkpoints_in_verification_window".to_string()
        )]
    );
}

#[test]
fn clean_run_yields_nothing() {
    assert!(parse_failures("PASS [ 1s] (1/1) suite test_one\nSummary ...").is_empty());
}
