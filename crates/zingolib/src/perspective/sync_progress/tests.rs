use json::JsonValue;
use zcash_protocol::consensus::BlockHeight;

use pepper_sync::sync::{ScanPriority, ScanRange, SyncResult, SyncStatus};

use super::{
    COMPLETE_PERCENTAGE, NO_PROGRESS_PERCENTAGE, NULLIFIER_RETRIEVAL_PERCENTAGE, SessionReport,
    SyncProgress,
};

const START: u32 = 1_000;
const END: u32 = 1_010;

fn status(
    blocks: (u32, u32),
    outputs: (u64, u64),
    priority: ScanPriority,
    started: bool,
) -> SyncStatus {
    SyncStatus {
        scan_ranges: vec![ScanRange::from_parts(
            BlockHeight::from_u32(START)..BlockHeight::from_u32(END),
            priority,
        )],
        sync_start_height: BlockHeight::from_u32(if started { START } else { 0 }),
        session_blocks_scanned: blocks.0,
        total_blocks_scanned: blocks.0,
        total_blocks: blocks.1,
        session_sapling_outputs_scanned: 0,
        total_sapling_outputs_scanned: 0,
        session_orchard_outputs_scanned: 0,
        total_orchard_outputs_scanned: 0,
        session_ironwood_outputs_scanned: 0,
        total_ironwood_outputs_scanned: 0,
        total_outputs_scanned: outputs.0,
        total_outputs: outputs.1,
    }
}

fn result(scanned: u64, total: u64, nullifier_retrieval_pending: bool) -> SyncResult {
    SyncResult {
        sync_start_height: BlockHeight::from_u32(START),
        sync_end_height: BlockHeight::from_u32(END - 1),
        blocks_scanned: END - START,
        sapling_outputs_scanned: 1,
        orchard_outputs_scanned: 2,
        ironwood_outputs_scanned: 3,
        total_outputs_scanned: scanned,
        total_outputs: total,
        nullifier_retrieval_pending,
    }
}

fn keys(value: JsonValue) -> Vec<String> {
    value.entries().map(|(key, _)| key.to_string()).collect()
}

/// HYPOTHESIS: a wallet that never started a sync reads as no progress
/// whatever its counts hold, so the falsifier is a never-started status
/// whose empty spans read as complete.
#[test]
fn a_never_started_sync_reads_as_no_progress() {
    let progress = SyncProgress::from(status((0, 0), (0, 0), ScanPriority::Historic, false));
    assert_eq!(
        progress.percentage_total_blocks_scanned,
        NO_PROGRESS_PERCENTAGE
    );
    assert_eq!(
        progress.percentage_total_outputs_scanned,
        NO_PROGRESS_PERCENTAGE
    );
}

/// HYPOTHESIS: each percentage is the exact ratio of its counts on the
/// percentage scale, so the falsifier is a percentage that disagrees
/// with the counts it was derived from.
#[test]
fn percentages_are_the_exact_ratios() {
    let progress = SyncProgress::from(status((5, 10), (15, 70), ScanPriority::Historic, true));
    assert_eq!(progress.percentage_total_blocks_scanned, 50.0);
    assert!(
        (progress.percentage_total_outputs_scanned - (15.0 / 70.0) * 100.0).abs() < f32::EPSILON
    );
}

/// HYPOTHESIS: a started sync over a span with nothing to scan reads as
/// complete rather than as an undefined ratio.
#[test]
fn an_empty_span_reads_as_complete_once_started() {
    let progress = SyncProgress::from(status((0, 0), (0, 0), ScanPriority::Scanned, true));
    assert_eq!(
        progress.percentage_total_blocks_scanned,
        COMPLETE_PERCENTAGE
    );
    assert_eq!(
        progress.percentage_total_outputs_scanned,
        COMPLETE_PERCENTAGE
    );
}

/// HYPOTHESIS: a complete span reads one step short while any range
/// awaits nullifier retrieval, and an incomplete span is left alone.
#[test]
fn pending_nullifiers_hold_a_complete_span_one_step_short() {
    let capped = SyncProgress::from(status(
        (10, 10),
        (70, 70),
        ScanPriority::ScannedWithoutMapping,
        true,
    ));
    assert_eq!(
        capped.percentage_total_blocks_scanned,
        NULLIFIER_RETRIEVAL_PERCENTAGE
    );
    assert_eq!(
        capped.percentage_total_outputs_scanned,
        NULLIFIER_RETRIEVAL_PERCENTAGE
    );

    let partial = SyncProgress::from(status(
        (5, 10),
        (35, 70),
        ScanPriority::RefetchingNullifiers,
        true,
    ));
    assert_eq!(partial.percentage_total_blocks_scanned, 50.0);
    assert_eq!(partial.percentage_total_outputs_scanned, 50.0);
}

/// HYPOTHESIS: a hand-built status whose scanned count exceeds its span
/// reads as complete, never above the scale.
#[test]
fn an_overrun_is_clamped_to_complete() {
    let progress = SyncProgress::from(status((12, 10), (75, 10), ScanPriority::Scanned, true));
    assert_eq!(
        progress.percentage_total_blocks_scanned,
        COMPLETE_PERCENTAGE
    );
    assert_eq!(
        progress.percentage_total_outputs_scanned,
        COMPLETE_PERCENTAGE
    );
}

/// HYPOTHESIS: the JSON keys are the ones pepper-sync rendered before the
/// derivation moved here, plus `total_blocks`, so every consumer that
/// decoded the old document decodes this one.
#[test]
fn progress_json_keeps_the_former_keys() {
    let progress = SyncProgress::from(status((5, 10), (15, 70), ScanPriority::Historic, true));
    assert_eq!(
        keys(JsonValue::from(progress)),
        [
            "scan_ranges",
            "sync_start_height",
            "session_blocks_scanned",
            "total_blocks_scanned",
            "total_blocks",
            "percentage_total_blocks_scanned",
            "session_sapling_outputs_scanned",
            "total_sapling_outputs_scanned",
            "session_orchard_outputs_scanned",
            "total_orchard_outputs_scanned",
            "session_ironwood_outputs_scanned",
            "total_ironwood_outputs_scanned",
            "percentage_total_outputs_scanned",
            "total_outputs_scanned",
            "total_outputs",
        ]
    );
}

/// HYPOTHESIS: the session report derives the session's total output
/// percentage by the same rule as the live status, nullifier cap
/// included.
#[test]
fn a_session_report_derives_by_the_same_rule() {
    assert_eq!(
        SessionReport::from(result(15, 60, false)).percentage_total_outputs_scanned,
        25.0
    );
    assert_eq!(
        SessionReport::from(result(60, 60, true)).percentage_total_outputs_scanned,
        NULLIFIER_RETRIEVAL_PERCENTAGE
    );
    assert_eq!(
        SessionReport::from(result(0, 0, false)).percentage_total_outputs_scanned,
        COMPLETE_PERCENTAGE
    );
}

/// HYPOTHESIS: the report renders the bytes `SyncResult` rendered before
/// the derivation moved here, which the CLI prints after every session.
#[test]
fn a_session_report_renders_the_former_text() {
    assert_eq!(
        SessionReport::from(result(60, 60, false)).to_string(),
        "Sync result:
{
    sync start height: 1000
    sync end height: 1009
    blocks scanned: 10
    sapling outputs scanned: 1
    orchard outputs scanned: 2
    ironwood outputs scanned: 3
    percentage total outputs scanned: 100
}"
    );
}

/// HYPOTHESIS: the report's JSON keeps the former keys and adds the two
/// totals it now carries.
#[test]
fn report_json_keeps_the_former_keys() {
    assert_eq!(
        keys(JsonValue::from(SessionReport::from(result(15, 60, false)))),
        [
            "sync_start_height",
            "sync_end_height",
            "blocks_scanned",
            "sapling_outputs_scanned",
            "orchard_outputs_scanned",
            "ironwood_outputs_scanned",
            "percentage_total_outputs_scanned",
            "total_outputs_scanned",
            "total_outputs",
        ]
    );
}
