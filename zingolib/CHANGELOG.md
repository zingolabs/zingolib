# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added

### Changed

### Removed

## [5.0.0] - 2026-06-08

### Added
- `grpc_connector::GetClientError`

### Changed
- `wallet::summary::data`:
  - `NoteSummary`: `output_index` field is now u32.
  - `OutgoingNoteSummary`: `output_index` field is now u32.
  - `CoinSummary`: `output_index` field is now u32.
  - `OutgoingCoinSummary`: `output_index` field is now u32.
- `wallet::disk`: serialized version incremented to 40 for serializing output indexes as u32.
- `wallet::output::OutputRef`: `output_index` method now returns u32.

## [4.0.0] - 2026-06-05

### Changed
- `lightclient::error::LightClientError`: added `SyncLaunchErrror` variant.
- `data::Receiver`: From impl for Payment is now a TryFrom

## [3.0.1] - 2026-03-26

## [3.0.0] - 2026-03-02

### Added
- `lightclient::error::TransmissionError` - moved from `wallet::error` and simplified to much fewer variants more specific
to transmission.
- `wallet`: publicly re-exported `pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, TransparentAddressDiscoveryScopes}`

### Changed
- `lightclient::LightClient::new`: no longer recommends the `chain_height` parameter to actually be {chain height - 100}. consumers should input the current chain height.
- `lightclient::error::LightClientError`:
  - `SyncError` fmt display altered
  - `SendError` variant added
  - `FileError` removed From impl for std::io::error
- `lightclient::error::SendError` - now includes all error types related to sending such as transmission and proposal errors.
- `wallet::LightWallet`:
  - removed `send_progress` field
  - `remove_unconfirmed_transactions` method renamed to `remove_failed_transactions` and now only removes transactions with the
new `Failed` status. Also now returns `wallet::error::WalletError`. No longer resets spends as spends are now reset when
a transaction is updated to `Failed` status. Transactions are automatically updated to `Failed` if transmission fails 4 times or
if the transaction expires before it is confirmed. Spends locked up in unconfirmed transactions for 3 blocks will also be reset
to release the funds, restoring balance and allowing funds to be spent in another transaction.
  - added `clear_proposal` method for removing an unconfirmed proposal from the wallet.
- `wallet::error::WalletError`:
  - added `ConversionFailed` variant
  - added `RemovalError` variant
  - added `TransactionNotFound` variant
  - added `TransactionRead` variant
  - added `BirthdayBelowSapling` variant
  - `TransactionWrite` removed From impl for std::io::error
  - `CalculateTxScanError` include fmt display of underlying error in fmt display
  - `ShardTreeError` fmt display altered
- `wallet::error::ProposeShieldError` - renamed `Insufficient` variant to `InsufficientFunds`
- `wallet::utils::interpret_memo_string`: changed name to `memo_bytes_from_string`. No longer decodes hex. Memo text will be displayed as inputted by the user.

### Removed
- `lightclient::LightClient::resend` - replaced by automatic retries due to issues with the current `resend` or `remove` user flow.
- `lightclient::LightClient::send_progress`
- `lightclient::error::QuickSendError`
- `lightclient::error::QuickShieldError`
- `lightclient::send_with_proposal` module - contents moved to `send` (parent) module.
- `wallet::send::SendProgress`
- `wallet::error::RemovalError` - variants added to `WalletError`
- `wallet::error::TransmissionError` - moved to `lightclient::error` module
- `error` module - unused

## [2.1.2] - 2026-01-14
