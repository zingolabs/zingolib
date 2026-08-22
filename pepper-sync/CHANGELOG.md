# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added

### Changed
- BREAKING: the truncation rescan announces itself. `error::SyncError::PoolHistoryReopened`
  is now a struct variant carrying the pool, the rescan height, the
  disagreeing block, and both tree sizes, and it renders one
  "RESCAN TRIGGERED" sentence naming the consequence and the cause.
  `error::ScanError::IncorrectTreeSize` gains the disagreeing block's
  `height`, and the truncation site logs at error level where it warned.
- `sync::sync_status`: a session with no scannable blocks or outputs no longer
  reports a finished sync. A stale initial sync state, whose previously scanned
  counts stand at or above the wallet's whole span, saturates the session
  denominator to zero. The status now reports the wallet's total progress for
  that session, where it previously divided zero by zero and coerced the
  resulting `NaN` to one hundred percent. The pool totals and the block span are
  saturated likewise, so a rewound tree bound can no longer underflow them.
- BREAKING: a wrapper error variant renders only its own layer. The
  `Display` texts of the `error::SyncError`, `error::MempoolError`, and
  `error::ScanError` wrapper variants no longer embed the wrapped source's
  text, and `ScanError::EncodingError` renders transparently. A consumer
  recovers the full failure story by walking the `source()` chain.
- `wallet::WalletTransaction::update_status`: added `fail_confirmed` bool for protecting against confirmed txs being
    set to failed in cases other than re-org truncation.

### Removed

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
