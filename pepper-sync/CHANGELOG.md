# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
- `wallet::SyncState::set_session_baseline_for_test` (behind `test-features`): places a
  sync state in the documented overshoot condition, where the session baseline has passed
  the span frozen at sync start, so consumers can regression-test their progress surfaces.

### Changed
- `wallet::WalletTransaction::update_status`: added `fail_confirmed` bool for protecting against confirmed txs being
    set to failed in cases other than re-org truncation.
- `sync::sync_status` no longer panics when scanning passes the tree bounds frozen at sync
  start. That condition is ordinary and is documented on
  `sync::SyncStatus::total_outputs_scanned`, but the session percentages divided by a span
  the baseline had already passed, which underflowed. In a debug build the panic aborted
  the calling thread; in a release build the subtraction wrapped and pinned every derived
  percentage at zero. The surviving session counts now saturate.

### Removed
- **Breaking.** `sync::SyncStatus::percentage_session_blocks_scanned` and
  `sync::SyncStatus::percentage_session_outputs_scanned`. These were the only fields whose
  computation could abort the process, and no consumer read them: the CLI's progress
  surfaces take `total_outputs_scanned`, `total_outputs`, and `is_complete()`, and
  `SyncResult` carries the total percentage only. A consumer that wants a session
  percentage computes it from the session and total counts the struct still reports. The
  two fields also leave the `json::JsonValue` rendering of a sync status.

## [0.5.0] - 2026-06-10

### Added
- `error::SyncRecoveryObservables` enum with variants `MaybeRecoverableServer`,
  `ServerUnavailable`, and `Abort` — classifies sync errors for consumer retry logic.
- `error::SyncError::is_retryable()` — returns `true` for transient errors
  (server timeouts, connection drops, mempool failures).
- `error::SyncError::recovery_recommendation()` — maps any sync error to a
  `SyncRecoveryObservables` without callers needing to match on error internals.
- `error::ServerError::is_retryable()` — distinguishes transport failures from
  invalid server data.
- `error::ServerError::recovery_recommendation()` — server-level recovery classification.

### Changed
- `wallet::TransparentCoin`: serialized version incremented to 1 to serialize output indexes as u32
- `wallet::WalletNote`: serialized version incremented to 2 to serialize output indexes as u32
- `wallet::OutgoingNote`: serialized version incremented to 1 to serialize output indexes as u32
- `wallet::OutputId`:
  - `output_index` field is now u32.
  - `new` constructor `output_index` parameter is now u32.
  - `output_index` method's return type is now u32.

## [0.4.0] - 2026-06-05

### Added
`wallet::WalletTransaction`: added `total_external_outgoing_note_value` method

## [0.3.0]

### Changed

- `sync::sync` fn: `client` parameter now takes a `CompactTxStreamerClient<tonic::Channel>`

## [0.2.0] - 2026-02-26

### Added
- `wallet::WalletTransaction::update_status`
- `wallet::WalletTransaction::new_for_test`
- `sync::set_transactions_failed` - also re-exported in lib.rs

### Changed
- `error::SyncError`:
  - added `BirthdayBelowSapling` variant which is returned when `sync` is called with wallet birthday below sapling activation height.
  - `ChainError` variant now includes the wallet height and chain height.
- `error::ScanError`:
  - `InvalidMemoBytes` variant now uses `zcash_protocol::memo::Error` instead of deprecated `zcash_primitives::memo::Error` type.
- `keys::KeyID` now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `keys::ScanningKeyOps` trait now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `keys::TransparentAddressId` now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `sync::ScanPriority`:
  - added `RefetchingNullifiers` variant.
- `wallet::SyncState`:
  - incremented to serialized version 3 to account for changes to `ScanPriority`
  - `wallet_height` method renamed to `last_known_chain_height`.
- `wallet::NoteInterface` trait: added `refetch_nullifier_ranges` method.
- `wallet::SaplingNote`:
  - implemented `refetch_nullifier_ranges` method.
  - updated serialization to account for new `WalletNote` field.
- `wallet::OrchardNote`:
  - implemented `refetch_nullifier_ranges` method.
  - updated serialization to account for new `WalletNote` field.
- `wallet::WalletNote`:
  - incremented to serialized version 1 to account for changes to `WalletNote` struct.

### Removed

## [0.1.0] - 2026-01-09
