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

### Changed
- `LightClient`:
  - `server_uri`: renamed `indexer_uri`
  - `set_server`: renamed `set_indexer_uri`
- `config::ChainType`: `Regtest` activation heights tuple variant field changed from zebra type to zingo common components type.
- `config::ZingoConfig`: reworked. public fields now private with public getter methods to constrain public API:
  - `wallet_dir` replaces `get_zingo_wallet_dir`
  - `network_type` method replaces `chain` field
  - `indexer_uri` method replaces `lightwalletd_uri` field and `get_lightwalletd_uri` method
  - `build` renamed `builder`
- `config::ZingoConfigBuilder`: reworked. public fields now private with public setter methods to constrain public API:
  - `create` renamed `build`

### Removed
- `regtest` feature: production binaries can now be tested in regtest mode.
- `config::ChainFromStingError`: replaced by `InvalidChainType` error struct.
- `config::chain_from_str`: replaced by impl TryFrom<&str> for `config::ChainType`
- `config::ZingoConfig`:
  - `get_wallet_with_name_pathbuf`
  - `get_wallet_with_name_path`
  - `wallet_with_name_path_exists`
  - `get_wallet_pathbuf`
  - `wallet_exists(`

## [4.0.0] - Reduce Read Locking

Restructures wallet ownership so that immutable metadata (chain type, birthday, mnemonic) is
stored outside the `RwLock`, enabling lock-free access to frequently-read fields. Also removes
the `log4rs`-based logging infrastructure from `zingolib`.

### Added
- `lightclient::ClientWallet`: new public struct wrapping `LightWallet` with immutable metadata
  stored outside the lock. Public constructors:
  - `ClientWallet::new(chain_type, birthday, mnemonic, wallet)`
  - `ClientWallet::from_wallet_base(network, wallet_base, birthday, wallet_settings)`
  - `ClientWallet::read(reader, network)` (deserialization, replaces `LightWallet::read`)
- `lightclient::LightClient`:
  - `chain_type()`: lock-free access to `ChainType`
  - `birthday()`: lock-free access to wallet birthday `BlockHeight`
  - `mnemonic()`: lock-free access to `Option<&Mnemonic>`
  - `wallet()`: returns `&Arc<RwLock<LightWallet>>`, replacing the former public field
- `wallet::WalletBase::resolve_keys(self, network)`: resolves a `WalletBase` into a
  `(BTreeMap<AccountId, UnifiedKeyStore>, Option<Mnemonic>)`. Logic was previously inlined
  inside `LightWallet::new`.
- `config::LightClientInitParams`: new public struct for initialization parameters.

### Changed
- `lightclient::LightClient`:
  - `pub wallet: Arc<RwLock<LightWallet>>` field replaced by `pub(crate) client_wallet: ClientWallet`.
    Use `client.wallet()` to obtain `&Arc<RwLock<LightWallet>>`.
  - `create_from_wallet` now takes `ClientWallet` instead of `LightWallet`.
- `wallet::LightWallet`:
  - `pub network: ChainType` field is now private. Use `LightClient::chain_type()`.
  - `pub birthday: BlockHeight` field is now private. Use `LightClient::birthday()`.
  - `pub fn read()` is now `pub(crate)`. Use `ClientWallet::read()` instead.
  - `pub fn mnemonic_phrase()` is now private.
- `wallet::disk::testing::examples::NetworkSeedVersion::load_example_wallet` returns
  `ClientWallet` instead of `LightWallet`.

### Removed
- `wallet::LightWallet::mnemonic()`: use `LightClient::mnemonic()` instead.
- `config::DEFAULT_LOGFILE_NAME` constant.
- `config::ZingoConfig`:
  - `logfile_name` field
  - `logfile_name()` method
  - `get_log_config()` method
  - `get_log_path()` method
- `config::ZingoConfigBuilder::set_logfile_name()` method.
- `log4rs` dependency removed from `zingolib` and workspace.

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
