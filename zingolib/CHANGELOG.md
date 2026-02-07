# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
`wallet::LightWallet::set_transaction_failed`
`lightclient::error::TransmissionError` - moved from `wallet::error` and simplified to much fewer variants more specific
to transmission.

### Changed
`lightclient::error::LightClientError`:
- `SyncError` fmt display altered
- `SendError` variant added
- `FileError` removed From impl for std::io::error
`lightclient::error::SendError` - now includes all error types related to sending such as transmission and proposal errors.
`wallet::LightWallet::remove_unconfirmed_transaction` - renamed to `remove_failed_transaction` and now only allows
transactions with the new `Failed` status to be removed. Also returns `WalletError` error type instead of `RemovalError`.
`wallet::LightWallet`:
- `remove_unconfirmed_transactions` method - renamed to `remove_failed_transactions` and now only removes transactions with the
- added `clear_proposal` method for removing an unconfirmed proposal from the wallet.
new `Failed` status. Also now returns `wallet::error::WalletError`. No longer resets spends as spends are now reset when
a transaction is updated to `Failed` status. 
`wallet::error::WalletError`:
- added `ConversionFailed` variant
- added `RemovalError` variant
- added `TransactionNotFound` variant
- added `TransactionRead` variant
- `TransactionWrite` removed From impl for std::io::error
- `CalculateTxScanError` include fmt display of underlying error in fmt display
- `ShardTreeError` fmt display altered
`wallet::error::ProposeShieldError` - renamed `Insufficient` variant to `InsufficientFunds`

### Removed
`lightclient::LightClient::resend` - replaced by automatic retries due to issues with the current `resend` or `remove` user flow.
`lightclient::LightClient::send_progress`  
`lightclient::error::QuickSendError`
`lightclient::error::QuickShieldError`
`lightclient::send_with_proposal` module - contents moved to `send` (parent) module.
`wallet::send::SendProgress`
`wallet::error::RemovalError` - variants added to `WalletError`
`wallet::error::TransmissionError` - moved to `lightclient::error` module
`error` module - unused

## [2.1.0] - 2025-12-18
