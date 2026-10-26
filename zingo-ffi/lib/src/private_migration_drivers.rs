#![cfg(feature = "unstable")]
#![forbid(unsafe_code)]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use json::object;
use zip32::AccountId;

use zingolib::lightclient::migrate;
use zingolib::wallet::migration;
use zingolib::wallet::migration::{parts, split};

use super::ZingolibError;

// Live progress of the in-flight execute batch, held outside the LIGHTCLIENT
// lock: `execute_due_parts` holds LIGHTCLIENT.write() for its whole
// `block_on`, so an `execute_status` poll reading the lightclient would
// deadlock behind it. `None` between batches.
static BATCH_PROGRESS: RwLock<Option<migrate::BatchProgressHandle>> = RwLock::new(None);

// Live progress of the in-flight note-splitting round, the split-path
// counterpart of BATCH_PROGRESS. `None` between rounds.
static SPLIT_PROGRESS: RwLock<Option<migrate::SplitProgressHandle>> = RwLock::new(None);

fn hash32_to_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_to_hash32(hex_str: &str) -> Option<[u8; 32]> {
    hex::decode(hex_str).ok()?.try_into().ok()
}

fn batch_report_json(report: &migrate::BatchReport) -> json::JsonValue {
    let outcomes = report
        .outcomes
        .iter()
        .map(|o| {
            let result = match &o.result {
                migrate::PartSendResult::Sent(txid) => object! {
                    "kind" => "sent",
                    "txid" => txid.to_string(),
                },
                migrate::PartSendResult::Slid => object! { "kind" => "slid" },
                migrate::PartSendResult::NotDue {
                    window_opens_unix_time,
                } => object! {
                    "kind" => "not_due",
                    "window_opens_unix_time" => *window_opens_unix_time,
                },
                migrate::PartSendResult::Failed { error } => object! {
                    "kind" => "failed",
                    "error" => error.clone(),
                },
            };
            object! {
                "part" => o.part.0,
                "denomination" => o.denomination,
                "result" => result,
            }
        })
        .collect::<Vec<_>>();
    object! {
        "outcomes" => outcomes,
        "halted" => match &report.halted {
            Some(e) => json::JsonValue::from(e.clone()),
            None => json::JsonValue::Null,
        },
    }
}

#[derive(uniffi::Object)]
pub struct PrivateMigrationDriver;

#[uniffi::export]
impl PrivateMigrationDriver {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// - Takes the lightclient read lock for the whole plan.
    pub fn plan(&self) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient_read(|lightclient| {
            super::RT.block_on(async move {
                let plan = lightclient
                    .plan_ironwood_migration(AccountId::ZERO)
                    .await
                    .map_err(super::ffi_error)?;
                let params = {
                    let wallet = lightclient.wallet().read().await;
                    migration::MigrationParams::provisional(wallet.chain_type())
                };
                let split_rounds = plan
                    .split_rounds
                    .iter()
                    .map(|round| {
                        round
                            .iter()
                            .map(|tx| {
                                object! {
                                    "inputs" => tx.inputs.clone(),
                                    "outputs" => tx.outputs.clone(),
                                    "fee" => tx.fee(),
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                Ok(object! {
                    "split_rounds" => split_rounds,
                    "parts" => plan.parts.clone(),
                    "split_fee" => plan.split_fee(),
                    "parts_fee" => plan.parts_fee(&params),
                    "residual" => plan.residual,
                    "plan_hash" => hash32_to_hex(&split::plan_hash(&plan)),
                }
                .pretty(2))
            })
        })
    }

    /// - Takes the lightclient write lock and persists the migration state.
    pub fn start(
        &self,
        plan_hash_hex: String,
        per_bucket: Option<u32>,
    ) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient(|lightclient| {
            let hash = hex_to_hash32(&plan_hash_hex)
                .ok_or_else(|| ZingolibError::InvalidInput("invalid plan hash".to_string()))?;
            super::RT.block_on(async move {
                lightclient
                    .start_ironwood_migration(
                        AccountId::ZERO,
                        parts::SigningStrategy::LazyAtBoundary,
                        hash,
                        per_bucket,
                    )
                    .await
                    .map_err(super::ffi_error)?;
                Ok(object! { "started" => true }.pretty(2))
            })
        })
    }

    /// - Takes the lightclient write lock for the whole round.
    /// - Pauses the running sync, then builds, proves and broadcasts one round of self-sends.
    pub fn continue_splitting(&self) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient(|lightclient| {
            super::RT.block_on(async move {
                let step = lightclient
                    .continue_note_splitting()
                    .await
                    .map_err(super::ffi_error)?;
                Ok(match step {
                    migrate::SplitStep::RoundTransmitted { round, txids } => object! {
                        "step" => "round_broadcast",
                        "round" => round,
                        "txids" => txids
                            .iter()
                            .map(|txid| txid.to_string())
                            .collect::<Vec<_>>(),
                    },
                    migrate::SplitStep::AwaitingConfirmation { pending } => object! {
                        "step" => "awaiting_confirmation",
                        "pending" => pending
                            .iter()
                            .map(|txid| txid.to_string())
                            .collect::<Vec<_>>(),
                    },
                    migrate::SplitStep::SplittingComplete => object! {
                        "step" => "splitting_complete",
                    },
                }
                .pretty(2))
            })
        })
    }

    /// - Takes the lightclient write lock for the whole round.
    /// - Pauses the running sync, builds and transmits one round, then resumes the sync.
    /// - Arms `SPLIT_PROGRESS` for the round and clears it afterwards.
    pub fn quick_split(&self) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient(|lightclient| {
            if let Ok(mut progress) = SPLIT_PROGRESS.write() {
                *progress = Some(lightclient.split_progress_handle());
            }
            let out = super::RT.block_on(async move {
                let outcome = lightclient
                    .quick_split(AccountId::ZERO, true)
                    .await
                    .map_err(super::ffi_error)?;
                Ok(match outcome {
                    migrate::SplitOutcome::Round { txids } => object! {
                        "outcome" => "round",
                        "txids" => txids
                            .iter()
                            .map(|txid| txid.to_string())
                            .collect::<Vec<_>>(),
                    },
                    migrate::SplitOutcome::AwaitingConfirmation => object! {
                        "outcome" => "awaiting_confirmation",
                    },
                    migrate::SplitOutcome::Complete => object! {
                        "outcome" => "complete",
                    },
                }
                .pretty(2))
            });
            if let Ok(mut progress) = SPLIT_PROGRESS.write() {
                *progress = None;
            }
            out
        })
    }

    /// - Takes the `SPLIT_PROGRESS` read lock, never the lightclient lock.
    pub fn split_status(&self) -> Result<String, ZingolibError> {
        super::with_panic_guard(|| {
            let status = {
                let progress = SPLIT_PROGRESS
                    .read()
                    .map_err(|_| ZingolibError::SideChannelPoisoned)?;
                progress.as_ref().and_then(|handle| handle.status())
            };
            Ok(match status {
                Some(s) => object! {
                    "total" => s.total,
                    "built" => s.built,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        migrate::SplitPhase::Building => "building",
                        migrate::SplitPhase::Transmitting => "transmitting",
                    },
                }
                .pretty(2),
                None => json::JsonValue::Null.pretty(2),
            })
        })
    }

    /// - Takes the lightclient write lock and rewrites the persisted schedule.
    pub fn reschedule_parts(&self, per_bucket: u32) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient(|lightclient| {
            super::RT.block_on(async move {
                lightclient
                    .reschedule_parts(per_bucket)
                    .await
                    .map_err(super::ffi_error)?;
                Ok(object! { "rescheduled" => true }.pretty(2))
            })
        })
    }

    /// - Takes the lightclient read lock.
    pub fn window_timeline(&self) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient_read(|lightclient| {
            super::RT.block_on(async move {
                let timeline = lightclient
                    .window_timeline()
                    .await
                    .map_err(super::ffi_error)?;
                Ok(match timeline {
                    Some(windows) => {
                        let reports = windows
                            .iter()
                            .map(|w| {
                                object! {
                                    "bucket_index" => w.bucket_index,
                                    "boundary" => u32::from(w.boundary),
                                    "close" => u32::from(w.close),
                                    "is_current" => w.is_current,
                                    "parts_total" => w.parts_total,
                                    "parts_confirmed" => w.parts_confirmed,
                                    "value_total" => w.value_total,
                                    "value_migrated" => w.value_migrated,
                                }
                            })
                            .collect::<Vec<_>>();
                        json::JsonValue::from(reports).pretty(2)
                    }
                    None => json::JsonValue::Null.pretty(2),
                })
            })
        })
    }

    /// - Takes the lightclient write lock for the whole batch.
    /// - Pauses the running sync, then signs and broadcasts every due part, `spacing_ms` apart.
    /// - Arms `BATCH_PROGRESS` for the batch and clears it afterwards.
    pub fn execute_due_parts(&self, spacing_ms: u64) -> Result<String, ZingolibError> {
        super::with_initialized_lightclient(|lightclient| {
            if let Ok(mut progress) = BATCH_PROGRESS.write() {
                *progress = Some(lightclient.batch_progress_handle());
            }
            let out = super::RT.block_on(async move {
                let report = lightclient
                    .execute_due_parts(Duration::from_millis(spacing_ms))
                    .await
                    .map_err(super::ffi_error)?;
                Ok(batch_report_json(&report).pretty(2))
            });
            if let Ok(mut progress) = BATCH_PROGRESS.write() {
                *progress = None;
            }
            out
        })
    }

    /// - Takes the `BATCH_PROGRESS` read lock, never the lightclient lock.
    pub fn execute_status(&self) -> Result<String, ZingolibError> {
        super::with_panic_guard(|| {
            let status = {
                let progress = BATCH_PROGRESS
                    .read()
                    .map_err(|_| ZingolibError::SideChannelPoisoned)?;
                progress.as_ref().and_then(|handle| handle.status())
            };
            Ok(match status {
                Some(s) => object! {
                    "total" => s.total,
                    "resolved" => s.resolved,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        migrate::BatchPhase::Sending => "sending",
                        migrate::BatchPhase::Spacing => "spacing",
                    },
                }
                .pretty(2),
                None => json::JsonValue::Null.pretty(2),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_round_trips_through_hex_and_back() {
        let hash: [u8; 32] = core::array::from_fn(|i| i as u8);
        let hex = hash32_to_hex(&hash);
        assert_eq!(hex.len(), 64);
        assert_eq!(hex_to_hash32(&hex), Some(hash));
    }

    #[test]
    fn uppercase_hex_decodes_to_the_same_hash() {
        let hash = [0xabu8; 32];
        let hex = hash32_to_hex(&hash).to_uppercase();
        assert_eq!(hex_to_hash32(&hex), Some(hash));
    }

    #[test]
    fn anything_but_64_hex_digits_is_rejected() {
        let valid = hash32_to_hex(&[0u8; 32]);
        let rejected = [
            String::new(),
            valid[..62].to_string(),
            valid[..63].to_string(),
            format!("{valid}00"),
            format!("zz{}", &valid[2..]),
            format!("+a{}", &valid[2..]),
            format!("é{}", &valid[2..]),
        ];
        for input in rejected {
            assert_eq!(
                hex_to_hash32(&input),
                None,
                "{input:?} must be rejected, not coerced into a consent hash"
            );
        }
    }

    #[test]
    fn plan_fails_typed_without_a_client() {
        let error = PrivateMigrationDriver::new()
            .plan()
            .expect_err("no test initializes the global lightclient, so plan cannot succeed");
        assert!(
            matches!(error, ZingolibError::LightclientNotInitialized),
            "plan must fail with the typed uninitialized variant: {error}"
        );
    }
}
