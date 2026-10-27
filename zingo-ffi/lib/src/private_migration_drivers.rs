#![cfg(feature = "unstable")]
#![forbid(unsafe_code)]

use std::panic::UnwindSafe;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use json::object;
use zip32::AccountId;

use zingolib::lightclient::LightClient;
use zingolib::lightclient::error::LightClientError;
use zingolib::lightclient::migrate;
use zingolib::wallet::migration;
use zingolib::wallet::migration::{parts, split};

use super::ZingolibError;

// Live progress of the in-flight execute batch, held outside the LIGHTCLIENT
// lock: `execute_due_parts` holds LIGHTCLIENT.write() for its whole
// `block_on`, so a `progress` poll reading the lightclient would
// deadlock behind it. `None` between batches.
static BATCH_PROGRESS: RwLock<Option<migrate::BatchProgressHandle>> = RwLock::new(None);

// Live progress of the in-flight note-splitting round, the split-path
// counterpart of BATCH_PROGRESS. `None` between rounds.
static SPLIT_PROGRESS: RwLock<Option<migrate::SplitProgressHandle>> = RwLock::new(None);

type Rendered = Result<json::JsonValue, LightClientError>;

fn pretty(rendered: Rendered) -> Result<String, ZingolibError> {
    rendered
        .map_err(super::ffi_error)
        .map(|json| json.pretty(2))
}

/// - Takes the lightclient read lock for the whole call.
fn read_json<F>(f: F) -> Result<String, ZingolibError>
where
    F: AsyncFnOnce(&LightClient) -> Rendered + UnwindSafe,
{
    super::with_initialized_lightclient_read(|lightclient| {
        pretty(super::RT.block_on(f(lightclient)))
    })
}

/// - Takes the lightclient write lock for the whole call.
/// - Arms `channel` with the handle `arm` draws from the lightclient, and clears it afterwards.
fn write_json<H, A, F>(channel: &RwLock<Option<H>>, arm: A, f: F) -> Result<String, ZingolibError>
where
    A: FnOnce(&LightClient) -> Option<H> + UnwindSafe,
    F: AsyncFnOnce(&mut LightClient) -> Rendered + UnwindSafe,
{
    super::with_initialized_lightclient(|lightclient| {
        let handle = arm(lightclient);
        if let Ok(mut slot) = channel.write() {
            *slot = handle;
        }
        let out = pretty(super::RT.block_on(f(lightclient)));
        if let Ok(mut slot) = channel.write() {
            *slot = None;
        }
        out
    })
}

/// - Takes the lightclient write lock for the whole call.
fn write_json_unarmed<F>(f: F) -> Result<String, ZingolibError>
where
    F: AsyncFnOnce(&mut LightClient) -> Rendered + UnwindSafe,
{
    write_json(&BATCH_PROGRESS, |_| None, f)
}

/// - Takes the read lock of `channel`, never the lightclient lock.
fn snapshot<H, S>(
    channel: &RwLock<Option<H>>,
    status: impl FnOnce(&H) -> Option<S>,
) -> Result<Option<S>, ZingolibError> {
    let progress = channel
        .read()
        .map_err(|_| ZingolibError::SideChannelPoisoned)?;
    Ok(progress.as_ref().and_then(status))
}

fn hash32_to_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_to_hash32(hex_str: &str) -> Option<[u8; 32]> {
    hex::decode(hex_str).ok()?.try_into().ok()
}

fn plan_json(plan: &split::MigrationPlan, params: &migration::MigrationParams) -> json::JsonValue {
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
    object! {
        "split_rounds" => split_rounds,
        "parts" => plan.parts.clone(),
        "split_fee" => plan.split_fee(),
        "parts_fee" => plan.parts_fee(params),
        "residual" => plan.residual,
        "plan_hash" => hash32_to_hex(&split::plan_hash(plan)),
    }
}

fn split_step_json(step: migrate::SplitStep) -> json::JsonValue {
    match step {
        migrate::SplitStep::RoundTransmitted { round, txids } => object! {
            "step" => "round_broadcast",
            "round" => round,
            "txids" => txids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        },
        migrate::SplitStep::AwaitingConfirmation { pending } => object! {
            "step" => "awaiting_confirmation",
            "pending" => pending.iter().map(ToString::to_string).collect::<Vec<_>>(),
        },
        migrate::SplitStep::SplittingComplete => object! {
            "step" => "splitting_complete",
        },
    }
}

fn split_outcome_json(outcome: migrate::SplitOutcome) -> json::JsonValue {
    match outcome {
        migrate::SplitOutcome::Round { txids } => object! {
            "outcome" => "round",
            "txids" => txids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        },
        migrate::SplitOutcome::AwaitingConfirmation => object! {
            "outcome" => "awaiting_confirmation",
        },
        migrate::SplitOutcome::Complete => object! {
            "outcome" => "complete",
        },
    }
}

fn part_result_json(result: &migrate::PartSendResult) -> json::JsonValue {
    match result {
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
    }
}

fn batch_report_json(report: &migrate::BatchReport) -> json::JsonValue {
    let outcomes = report
        .outcomes
        .iter()
        .map(|o| {
            object! {
                "part" => o.part.0,
                "denomination" => o.denomination,
                "result" => part_result_json(&o.result),
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
        read_json(async |lightclient| {
            let plan = lightclient.plan_ironwood_migration(AccountId::ZERO).await?;
            let wallet = lightclient.wallet().read().await;
            let params = migration::MigrationParams::provisional(wallet.chain_type());
            Ok(plan_json(&plan, &params))
        })
    }

    /// - Takes the lightclient write lock and persists the migration state.
    pub fn start(
        &self,
        plan_hash_hex: String,
        per_bucket: Option<u32>,
    ) -> Result<String, ZingolibError> {
        let hash = hex_to_hash32(&plan_hash_hex)
            .ok_or_else(|| ZingolibError::InvalidInput("invalid plan hash".to_string()))?;
        write_json_unarmed(async |lightclient| {
            lightclient
                .start_ironwood_migration(
                    AccountId::ZERO,
                    parts::SigningStrategy::LazyAtBoundary,
                    hash,
                    per_bucket,
                )
                .await?;
            Ok(object! { "started" => true })
        })
    }

    /// - Takes the lightclient write lock for the whole round.
    /// - Pauses the running sync, then builds, proves and broadcasts one round of self-sends.
    pub fn continue_splitting(&self) -> Result<String, ZingolibError> {
        write_json_unarmed(async |lightclient| {
            Ok(split_step_json(
                lightclient.continue_note_splitting().await?,
            ))
        })
    }

    /// - Takes the lightclient write lock for the whole round.
    /// - Pauses the running sync, builds and transmits one round, then resumes the sync.
    /// - Arms `SPLIT_PROGRESS` for the round and clears it afterwards.
    pub fn quick_split(&self) -> Result<String, ZingolibError> {
        write_json(
            &SPLIT_PROGRESS,
            |lightclient| Some(lightclient.split_progress_handle()),
            async |lightclient| {
                let outcome = lightclient.quick_split(AccountId::ZERO, true).await?;
                Ok(split_outcome_json(outcome))
            },
        )
    }

    /// - Takes the read locks of `SPLIT_PROGRESS` and `BATCH_PROGRESS`, never the lightclient lock.
    pub fn progress(&self) -> Result<String, ZingolibError> {
        super::with_panic_guard(|| {
            let split = snapshot(&SPLIT_PROGRESS, migrate::SplitProgressHandle::status)?;
            let batch = snapshot(&BATCH_PROGRESS, migrate::BatchProgressHandle::status)?;
            Ok(object! {
                "split" => split.map_or(json::JsonValue::Null, |s| {
                    object! {
                        "total" => s.total,
                        "built" => s.built,
                        "sent" => s.sent,
                        "phase" => match s.phase {
                            migrate::SplitPhase::Building => "building",
                            migrate::SplitPhase::Transmitting => "transmitting",
                        },
                    }
                }),
                "batch" => batch.map_or(json::JsonValue::Null, |s| {
                    object! {
                        "total" => s.total,
                        "resolved" => s.resolved,
                        "sent" => s.sent,
                        "phase" => match s.phase {
                            migrate::BatchPhase::Sending => "sending",
                            migrate::BatchPhase::Spacing => "spacing",
                        },
                    }
                }),
            }
            .pretty(2))
        })
    }

    /// - Takes the lightclient write lock and rewrites the persisted schedule.
    pub fn reschedule_parts(&self, per_bucket: u32) -> Result<String, ZingolibError> {
        write_json_unarmed(async |lightclient| {
            lightclient.reschedule_parts(per_bucket).await?;
            Ok(object! { "rescheduled" => true })
        })
    }

    /// - Takes the lightclient read lock.
    pub fn window_timeline(&self) -> Result<String, ZingolibError> {
        read_json(async |lightclient| {
            Ok(match lightclient.window_timeline().await? {
                Some(windows) => windows
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
                    .collect::<Vec<_>>()
                    .into(),
                None => json::JsonValue::Null,
            })
        })
    }

    /// - Takes the lightclient write lock for the whole batch.
    /// - Pauses the running sync, then signs and broadcasts every due part, `spacing_ms` apart.
    /// - Arms `BATCH_PROGRESS` for the batch and clears it afterwards.
    pub fn execute_due_parts(&self, spacing_ms: u64) -> Result<String, ZingolibError> {
        write_json(
            &BATCH_PROGRESS,
            |lightclient| Some(lightclient.batch_progress_handle()),
            async |lightclient| {
                let report = lightclient
                    .execute_due_parts(Duration::from_millis(spacing_ms))
                    .await?;
                Ok(batch_report_json(&report))
            },
        )
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

    #[test]
    fn a_bad_plan_hash_is_refused_before_the_lock_is_taken() {
        let error = PrivateMigrationDriver::new()
            .start("zz".to_string(), None)
            .expect_err("two non-hex characters are not a consent hash");
        assert!(matches!(error, ZingolibError::InvalidInput(_)), "{error}");
    }
}
