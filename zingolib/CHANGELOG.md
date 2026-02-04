# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
`wallet::LightWallet::set_transaction_failed`

### Changed
`wallet::LightWallet::remove_unconfirmed_transaction` - renamed to `remove_failed_transaction` and now only allows
transactions with the new `Failed` status to be removed. Also returns `WalletError` error type instead of `RemovalError`.
`wallet::error::WalletError` - variants added from `RemovalError`

### Removed
`LightClient::resend` - replaced by automatic retries due to issues with the current `resend` or `remove` user flow.
`wallet::error::RemovalError` - variants added to `WalletError`

## [2.1.0] - 2025-12-18
