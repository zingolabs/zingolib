# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated
- `config::load_clientconfig`: being replaced by zingo config builder pattern (`ZingoConfigBuilder`)

### Added
- impl TryFrom<&str> for `config::ChainType`
- `config::InvalidChainType`
- `lightclient::WalletMeta`: new public struct wrapping `LightWallet` with metadata and immutable wallet data
  stored outside the lock.
- `lightclient::LightClient`:
  - `chain_type` method: lock-free access to `ChainType`
  - `birthday` method: lock-free access to wallet birthday `BlockHeight`
  - `mnemonic_phrase` method: lock-free access to the wallet's mnemonic phrase
  - `wallet` method: returns `&Arc<RwLock<LightWallet>>`, replacing the former public field
  - `indexer: GrpcIndexer` field: owning the indexer connection directly
- re-export `zingo_common_components::protocol::ActivationHeights` so test crates can unify zingo common types with
  zingolib lightclient construction 

### Changed
- `lightclient::LightClient`:
  - `server_uri`: renamed `indexer_uri`
  - `set_server`: renamed `set_indexer_uri`
  - `pub wallet: Arc<RwLock<LightWallet>>` field is now private. replaced by `wallet` method.
- `config` module:
  - `ChainType`: `Regtest` activation heights tuple variant field changed from zebra type to zingo common components type.
  - `ZingoConfig`: reworked. public fields now private with public getter methods to constrain public API:
    - `wallet_dir` replaces `get_zingo_wallet_dir`
    - `chain_type` method replaces `chain` field
    - `indexer_uri` method replaces `lightwalletd_uri` field and `get_lightwalletd_uri` method
    - `build` renamed `builder`
  - `ZingoConfigBuilder`: reworked. public fields now private with public setter methods to constrain public API:
    - `create` renamed `build`
  - `DEFAULT_LIGHTWALLETD_SERVER` const: renamed `DEFAULT_INDEXER_URI`
  - `DEFAULT_TESTNET_LIGHTWALLETD_SERVER` const: renamed `DEFAULT_INDEXER_URI_TESTNET`
  - `DEVELOPER_DONATION_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_DONATION_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_TESTNET_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_REGTEST_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_AMOUNT` const: moved to lib.rs
  - `get_donation_address_for_chain` fn moved to lib.rs and renamed `get_zennies_for_zingo_address`
      now takes `ChainType` instead of `&ChainType`
  - `construct_lightwalletd_uri` fn: now returns result for handling URI errors
- `wallet::LightWallet`:
  - `pub network: ChainType` field is now private. Use `LightClient::chain_type()`.
  - `pub birthday: BlockHeight` field is now private. Use `LightClient::birthday()`.
  - new wallet serialization version 40 due to changes to chain type fmt::Display. chain type is now encoded as u8.
- `wallet::keys::unified::UnifiedKeyStore`:
  - `new_from_seed` method: now takes `ChainType` instead of `&ChainType`
  - `new_from_mnemonic` method: now takes `ChainType` instead of `&ChainType`
  - `new_from_ufvk` method: now takes `ChainType` instead of `&ChainType`

### Removed
- `log4rs` dependency
- `config::DEFAULT_LOGFILE_NAME` constant
- `config::ZingoConfig`:
  - `logfile_name` method
  - `get_log_config` method
  - `get_log_path` method
- `config::ZingoConfigBuilder`:
  - `set_logfile_name` method
- `regtest` feature: production binaries can now be tested in regtest mode.
- `config::ChainFromStingError`: replaced by `InvalidChainType` error struct.
- `config::chain_from_str`: replaced by impl TryFrom<&str> for `config::ChainType`
- `config::ZingoConfig`:
  - `get_wallet_with_name_pathbuf`
  - `get_wallet_with_name_path`
  - `wallet_with_name_path_exists`
  - `get_wallet_pathbuf`
  - `wallet_exists(`
- `config::DEFAULT_LOGFILE_NAME` constant.
- `config::ZingoConfig`:
  - `logfile_name` field
  - `logfile_name()` method
  - `get_log_config()` method
  - `get_log_path()` method
- `config::ZingoConfigBuilder::set_logfile_name()` method.
- `wallet::LightWallet::mnemonic()`

## [3.0.0] - 2026-03-02

### Deprecated

### Added
- `lightclient::error::TransmissionError`: moved from `wallet::error` and simplified to much fewer variants more specific
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
