#![forbid(unsafe_code)]

use json::JsonValue;
use zcash_protocol::consensus::BlockHeight;

use pepper_sync::sync::{SyncResult, SyncStatus};

const PERCENTAGE_SCALE: f32 = 100.0;
const COMPLETE_PERCENTAGE: f32 = PERCENTAGE_SCALE;
const NO_PROGRESS_PERCENTAGE: f32 = 0.0;
const PERCENTAGE_STEP: f32 = 1.0;
const NULLIFIER_RETRIEVAL_PERCENTAGE: f32 = COMPLETE_PERCENTAGE - PERCENTAGE_STEP;
const NEVER_STARTED: BlockHeight = BlockHeight::from_u32(0);

fn percentage_scanned(scanned: u64, total: u64) -> Option<f32> {
    (total != 0).then(|| {
        ((scanned as f32 / total as f32) * PERCENTAGE_SCALE)
            .clamp(NO_PROGRESS_PERCENTAGE, COMPLETE_PERCENTAGE)
    })
}

fn progress_percentage(scanned: u64, total: u64, nullifier_retrieval_pending: bool) -> f32 {
    let percentage = percentage_scanned(scanned, total).unwrap_or(COMPLETE_PERCENTAGE);
    if nullifier_retrieval_pending && percentage == COMPLETE_PERCENTAGE {
        NULLIFIER_RETRIEVAL_PERCENTAGE
    } else {
        percentage
    }
}

/// ```
/// use pepper_sync::sync::{ScanPriority, ScanRange, SyncStatus};
/// use zcash_protocol::consensus::BlockHeight;
/// use zingolib::perspective::sync_progress::SyncProgress;
///
/// let status = SyncStatus {
///     scan_ranges: vec![ScanRange::from_parts(
///         BlockHeight::from_u32(1_000)..BlockHeight::from_u32(1_010),
///         ScanPriority::Scanned,
///     )],
///     sync_start_height: BlockHeight::from_u32(1_000),
///     session_blocks_scanned: 5,
///     total_blocks_scanned: 5,
///     total_blocks: 10,
///     session_sapling_outputs_scanned: 0,
///     total_sapling_outputs_scanned: 0,
///     session_orchard_outputs_scanned: 15,
///     total_orchard_outputs_scanned: 15,
///     session_ironwood_outputs_scanned: 0,
///     total_ironwood_outputs_scanned: 0,
///     total_outputs_scanned: 15,
///     total_outputs: 60,
/// };
/// let progress = SyncProgress::from(status);
/// assert_eq!(progress.percentage_total_blocks_scanned, 50.0);
/// assert_eq!(progress.percentage_total_outputs_scanned, 25.0);
/// assert_eq!(progress.to_string(), "percentage complete: 25");
/// ```
#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub status: SyncStatus,
    pub percentage_total_blocks_scanned: f32,
    pub percentage_total_outputs_scanned: f32,
}

impl From<SyncStatus> for SyncProgress {
    fn from(status: SyncStatus) -> Self {
        let (percentage_total_blocks_scanned, percentage_total_outputs_scanned) = if status
            .sync_start_height
            == NEVER_STARTED
        {
            (NO_PROGRESS_PERCENTAGE, NO_PROGRESS_PERCENTAGE)
        } else {
            let pending = status.nullifier_retrieval_pending();
            (
                progress_percentage(
                    u64::from(status.total_blocks_scanned),
                    u64::from(status.total_blocks),
                    pending,
                ),
                progress_percentage(status.total_outputs_scanned, status.total_outputs, pending),
            )
        };
        Self {
            status,
            percentage_total_blocks_scanned,
            percentage_total_outputs_scanned,
        }
    }
}

impl std::fmt::Display for SyncProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "percentage complete: {}",
            self.percentage_total_outputs_scanned
        )
    }
}

impl From<SyncProgress> for JsonValue {
    fn from(progress: SyncProgress) -> Self {
        let status = progress.status;
        let scan_ranges: Vec<JsonValue> = status
            .scan_ranges
            .iter()
            .map(|range| {
                json::object! {
                    "priority" => format!("{:?}", range.priority()),
                    "start_block" => range.block_range().start.to_string(),
                    "end_block" => (range.block_range().end - 1).to_string(),
                }
            })
            .collect();

        json::object! {
            "scan_ranges" => scan_ranges,
            "sync_start_height" => u32::from(status.sync_start_height),
            "session_blocks_scanned" => status.session_blocks_scanned,
            "total_blocks_scanned" => status.total_blocks_scanned,
            "total_blocks" => status.total_blocks,
            "percentage_total_blocks_scanned" => progress.percentage_total_blocks_scanned,
            "session_sapling_outputs_scanned" => status.session_sapling_outputs_scanned,
            "total_sapling_outputs_scanned" => status.total_sapling_outputs_scanned,
            "session_orchard_outputs_scanned" => status.session_orchard_outputs_scanned,
            "total_orchard_outputs_scanned" => status.total_orchard_outputs_scanned,
            "session_ironwood_outputs_scanned" => status.session_ironwood_outputs_scanned,
            "total_ironwood_outputs_scanned" => status.total_ironwood_outputs_scanned,
            "percentage_total_outputs_scanned" => progress.percentage_total_outputs_scanned,
            "total_outputs_scanned" => status.total_outputs_scanned,
            "total_outputs" => status.total_outputs,
        }
    }
}

/// ```
/// use pepper_sync::sync::SyncResult;
/// use zcash_protocol::consensus::BlockHeight;
/// use zingolib::perspective::sync_progress::SessionReport;
///
/// let result = SyncResult {
///     sync_start_height: BlockHeight::from_u32(1_000),
///     sync_end_height: BlockHeight::from_u32(1_009),
///     blocks_scanned: 10,
///     sapling_outputs_scanned: 1,
///     orchard_outputs_scanned: 2,
///     ironwood_outputs_scanned: 3,
///     total_outputs_scanned: 60,
///     total_outputs: 60,
///     nullifier_retrieval_pending: false,
/// };
/// let report = SessionReport::from(result);
/// assert_eq!(report.percentage_total_outputs_scanned, 100.0);
/// assert!(report.to_string().starts_with("Sync result:"));
/// ```
#[derive(Debug, Clone)]
pub struct SessionReport {
    pub result: SyncResult,
    pub percentage_total_outputs_scanned: f32,
}

impl From<SyncResult> for SessionReport {
    fn from(result: SyncResult) -> Self {
        let percentage_total_outputs_scanned = progress_percentage(
            result.total_outputs_scanned,
            result.total_outputs,
            result.nullifier_retrieval_pending,
        );
        Self {
            result,
            percentage_total_outputs_scanned,
        }
    }
}

impl std::fmt::Display for SessionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sync result:
{{
    sync start height: {}
    sync end height: {}
    blocks scanned: {}
    sapling outputs scanned: {}
    orchard outputs scanned: {}
    ironwood outputs scanned: {}
    percentage total outputs scanned: {}
}}",
            self.result.sync_start_height,
            self.result.sync_end_height,
            self.result.blocks_scanned,
            self.result.sapling_outputs_scanned,
            self.result.orchard_outputs_scanned,
            self.result.ironwood_outputs_scanned,
            self.percentage_total_outputs_scanned,
        )
    }
}

impl From<SessionReport> for JsonValue {
    fn from(report: SessionReport) -> Self {
        let result = report.result;
        json::object! {
            "sync_start_height" => u32::from(result.sync_start_height),
            "sync_end_height" => u32::from(result.sync_end_height),
            "blocks_scanned" => result.blocks_scanned,
            "sapling_outputs_scanned" => result.sapling_outputs_scanned,
            "orchard_outputs_scanned" => result.orchard_outputs_scanned,
            "ironwood_outputs_scanned" => result.ironwood_outputs_scanned,
            "percentage_total_outputs_scanned" => report.percentage_total_outputs_scanned,
            "total_outputs_scanned" => result.total_outputs_scanned,
            "total_outputs" => result.total_outputs,
        }
    }
}

#[cfg(test)]
mod tests;
