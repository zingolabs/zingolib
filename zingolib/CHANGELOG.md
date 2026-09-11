# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [6.0.0] - 2026-09-07

### Added
- Add `LightClient::from_bytes` to build a client from in-memory wallet bytes.
- Add ZIP 318 Orchard to Ironwood migration in `lightclient::migrate`: `plan_immediate_migration`, `quick_immediate_migration`, `plan_note_split`, `quick_split`, `plan_ironwood_migration`, `start_ironwood_migration`, `execute_due_parts`, `transmit_due_parts`, `auto_transmit_if_due`, `reconcile_migration`, `catch_up_migration`, `reschedule_parts`, `cancel_ironwood_migration`, `migration_status`, `window_timeline`, `split_progress_handle`, `batch_progress_handle`.
- Add `wallet::migration` (plans, parts, denominations, buckets, schedule, persisted state). The wallet file's migration section carries its own version, 4.
- Add `ironwood_notes`, `outgoing_ironwood_notes` and `is_orchard_to_ironwood_migration` to summaries.
- Add `mixnet` module and the off-by-default `nym` feature for Nym mixnet transport.
- Add `LightClient::enable_mixnet`, `enable_mixnet_via_host` (via `mixnet::acquire::ProxyHosting`) and `attach_mixnet`, all returning `mixnet::acquire::TransportError`.
- Add `mixnet::Indicator` with six states, including `PreviouslyProvenThisEpoch`, and `LightClient::read_mixnet_indicator`.
- Add `mixnet::MixnetConduit`, `LightClient::mixnet_conduit`, `mixnet::resolve_route` and `MixnetRoute`.
- Add Proven Client acquisition: every client must complete a Sentinel check before first use. When all attempts fail, acquisition returns `TransportError::NoProvenExit`.
- Add `NodeHealthIndex` of per-exit `Proven` / `Failed` observations to order Clutch draws.
- Add Standing Client failover and a rotation watchdog on `[CLIENT_ROTATION_MIN, CLIENT_ROTATION_MAX]`, driven by `rotation_verdict` on `ProxyHosting` and `TransportAcquirable`.
- Add `client_rotation_min` and `client_rotation_max` to `MixnetTiming`.
- Add `mixnet::speed` (`SpeedPrioritized`, `run_speed_prioritized`, `MAX_SPEED_EXIT_DRAWS`) and `lightclient::select::SURVEY_WAVE_WIDTH`.
- Add the Server-Selection Sweep: an unpinned online session selects its sync indexer from the indexers that responded as healthy. `go_offline` aborts it.
- Add `SurveyResult::refusal` and `SweepError::EmptyCohort::causes`.
- Add `zingolib::destination` module: `Correspondable`, `Host`, `Operator`, `NoEligibleDestinations`, and the session Exit Pool.
- Add hedged send escalation: `TRANSMISSION_HEDGE_INTERVAL`, at most `RESERVATION_CLUTCH_SIZE` pulls in flight, six Destinations at most.
- Add `OutputLockStore` implementation for `LightWallet`.
- Add `lightclient::SaveShutdown` (`ShutDown`, `NotRunning`).
- Add `LightClientError::ProbeRequiresMixnet` and `MixnetProxyError::NoExits`.
- Add typed `mixnet::ExitNodeId` (`parse`, `TryFrom<String>`, `BlankExitNodeId`).
- Add `MixnetPriceFetch::route: PriceFetchRoute` (`Mixnet` or `Clearnet`).
- Add `NetOpStage::ProxyLaunch` death detail for a `nym-proxy` that dies before its stdout protocol.
- Add `zingo_netutils::Socks5Indexer::get_latest_block`.
- Add `perspective` feature (off by default) and `zingolib::perspective` module.

### Changed
- **Breaking:** bump `zcash_primitives` and `zcash_proofs` to 0.30, `zcash_transparent` to 0.10, `zcash_keys` to 0.16, `zcash_client_backend` to 0.24.0-rc.7, and `zcash_pool_migration` to the published 0.1.0-rc.7.
- **Breaking:** source ZIP 318 constants from `zcash_protocol::zip318`, set `ANCHOR_AGE_CAP` to 4, and set the transfer-delay mean to 66 blocks.
- **Breaking:** rename `MIGRATION_MAX_DENOMINATION_ZEC` to `DENOM_CAP` and `RESIDUAL_MIGRATION_MIN` to `MAX_RESIDUAL_VALUE`, both `Zatoshis`.
- **Breaking:** rename "broadcast" / "witness" to "transmission" / "destination": `migration_broadcast_uri` to `migration_transmission_uri`, `BroadcastClient` to `TransmissionClient`, `BroadcastError` to `PartTransmissionError`, `BroadcastWindow` to `TransmissionWindow`, `SplitStep::RoundBroadcast` to `RoundTransmitted`, `MigrationBroadcastTargetIsSyncEndpoint` to `MigrationTransmissionTargetIsSyncEndpoint`, `NoEligibleBroadcastIndexer` to `NoEligibleDestination`, `probe_broadcast_indexers` to `probe_destinations`, `broadcast_due_parts` to `transmit_due_parts`, `auto_broadcast_if_due` to `auto_transmit_if_due`, `TransmitRoute::Mixnet::witness` to `destination`, `mixnet::broadcast` to `mixnet::destination_rotation`, `lightclient::migrate::{broadcast_grpc, broadcast_route}` to `{transmission_grpc, transmission_route}`.
- **Breaking:** remove the default server: `config::construct_indexer_uri` takes `String`.
- **Breaking:** rename `mixnet::MixnetMode` to `mixnet::Indicator`, `UnknownMixnetModeToken` to `UnknownIndicatorToken`, `LightClient::mixnet_mode` to `read_mixnet_indicator`.
- **Breaking:** rename `MAX_DIARY_ATTEMPTS` to `MAX_HISTORY_ATTEMPTS`.
- **Breaking:** route `LightClient::update_current_price` over clearnet when Mixnet Mode is switched off, and refuse with `MixnetNotReady` in `Unattached`, `Bootstrapping` and `Died`.
- **Breaking:** stop writing the fetched price to the wallet. The price is returned only in `MixnetPriceFetch`.
- **Breaking:** compile the price fetch only with the `nym` feature.
- **Breaking:** move `ProxyHosting`, `HostedTransport`, `HostRefusal` and `HostedProvider` to `zingo_netutils::provider`, re-exported from `mixnet::acquire`.
- **Breaking:** make `wallet::migration::parts::ProveOnce` a struct, held as `Box<ProveOnce>` in `PrepareResult::Ready::prove`.
- **Breaking:** type the SOCKS5 endpoint as `std::net::SocketAddr` throughout, including `switch_on_mixnet_for_tests`, and indexer endpoints as `destination::Host`.
- **Breaking:** carry error sources as `source()` links instead of embedded text in `LightClientError`, `SendError`, `PriceError` and `TransportError`.
- **Breaking:** reduce `lightclient::select::ServerSelectionError` to `Speed` and `Selection`.
- **Breaking:** add `PriceError::Speed`.
- **Breaking:** move `ValueTransfer`, the finsight rollups and the `value_transfers` / `messages_containing` / `finsight` / `do_total_*` methods to `zingolib::perspective`. The `testutils` feature enables `perspective`.
- **Breaking:** keep Health always on, in memory and session-scoped. `IndexerAttempt` gains `phase`.
- **Breaking:** bind one exclusive exit per Transmission pull on a spawned session.
- Change `config::ClientConfigBuilder::build` to return a `Result`.
- Change the wallet file format to version 42. Versions 32 to 43 are read.
- Classify a transport failure whose text says its deadline elapsed as a timeout.

### Removed
- **Breaking:** remove the on-disk indexer diary, the `nym-diary` feature, `LightClient::set_indexer_diary`, `IndexerHistoryHandle::{beside_wallet, is_recording}` and `IndexerAttempt::exit`.
- **Breaking:** remove responsiveness classes: the type parameter on `enable_mixnet`, and the `mixnet` re-export of `PrioritisePrivacy`, `PrioritiseSpeed`, `Responsiveness`.
- **Breaking:** remove Destination Pool member-keeping and go-online background refills.
- **Breaking:** remove `DEFAULT_INDEXER_URI` and `DEFAULT_INDEXER_URI_TESTNET`.
- Remove `LightClientError::PriceFetchRequiresMixnet`.
- Remove `MixnetRoute::socks5_proxy`.
- Remove `mixnet::IP_CORRELATION_DISCLAIMER`.
- Remove `mixnet::sweep::{indexer_lanes, opening_wave_timed_out}`.
- Remove `wallet::LightWallet::update_current_price`.
- Remove `TransactionSummary::balance_delta`, `TransactionSummaries::paid_fees` and `TransactionSummaries::txids` (#2612).

## [5.0.0] - 2026-06-10

### Added
- `lightclient::LightClient::poll_sync_recovery()` — polls the sync task and,
  if it failed, returns `(SyncRecoveryObservables, String)` with the recommended
  recovery action and error description. Primary entry point for consumers
  (CLI, mobile, PC) to handle sync failures.
- impl TryFrom<&str> for `config::ChainType`
- `config::InvalidChainType`
- `lightclient::WalletMeta`: new public struct wrapping `LightWallet` with metadata and immutable wallet data
  stored outside the lock.
- `lightclient::LightClient`:
  - `chain_type` method: lock-free access to `ChainType`
  - `birthday` method: lock-free access to wallet birthday as `u32`
  - `mnemonic_phrase` method: lock-free access to the wallet's mnemonic phrase
  - `wallet_path` method returns wallet file path
  - `wallet_dir` method returns path to directory which holds wallet file
  - `wallet` method: returns `&Arc<RwLock<LightWallet>>`, replacing the former public field
  - `indexer: GrpcIndexer` field: owning the indexer connection directly
  - `backup_wallet_file` method to replace `ZingoConfig` method
  - updated `Debug` impl
- re-export `zingo_common_components::protocol::ActivationHeights` so test crates can unify zingo common types with
  zingolib lightclient construction
- `wallet::utils`: added `get_zcash_params_path` fn to replace `ZingoConfig` method.
- `config::WalletConfig` enum: replaces functionality of `wallet::WalletBase`. now encapsulates all wallet config for creation
  of a `wallet::Lightwallet` for each variant i.e. from seed or ufvk
- `testutils::default_test_wallet_settings`
- `wallet::WalletSettings`: `default` impl

### Changed
- Upgraded `zingo-netutils` from 3.0.0 to 5.0.1:
  - proto types now come from `lightwallet-protocol` via `zingo_netutils::lightwallet_protocol`.
  - `globally-public-transparent` feature gates are enabled.
- `lightclient::LightClient`:
  - `new` now installs the rustls ring crypto provider (idempotent) since
    `GrpcIndexer::new` pre-builds a TLS endpoint at construction time.
  - `indexer_uri` now returns `&http::Uri` instead of `Option<&http::Uri>`.
  - `set_indexer_uri` now returns `Result<(), zingo_netutils::GetClientError>` and
    constructs a new `GrpcIndexer` internally (`set_uri` was removed upstream).
  - `server_uri`: renamed `indexer_uri`
  - `set_server`: renamed `set_indexer_uri`
  - `pub wallet: Arc<RwLock<LightWallet>>` field is now private. replaced by `wallet` method.
  - `new` constructor: removed `chain_height` parameter which is now within the config
- `lightclient::error::LightClientError`: removed `TorClientError` variant.
- `config` module:
  - `ChainType`:
    - `Regtest` activation heights tuple variant field changed from zebra type to zingo common components type.
    - `fmt::Display` impl changed to give full network type names.
    - `zcash_protocol::consensus::Parameters` impl is no longer public to constrain external types in public API.
  - `ZingoConfig`:
    - renamed: `ClientConfig`
    - `wallet_settings` and `no_of_accounts` fields replaced by `wallet_config` field
    - `network_type` field renamed `chain_type`
    - reworked. public fields now private with public getter methods to constrain public API:
      - `wallet_dir` replaces `get_zingo_wallet_dir`
      - `chain_type` method replaces `chain` field
      - `indexer_uri` method replaces `lightwalletd_uri` field and `get_lightwalletd_uri` method
      - `build` renamed `builder`
      - `wallet_settings` and `no_of_accounts` methods replaced by `wallet_config` method
      - `get_zcash_params_path` replaced by `utils::get_zcash_params_path` fn
      - `backup_existing_wallet` replaced by `LightClient::backup_wallet_file`
  - `ClientConfigBuilder::build`: default `indexer_uri` is now `DEFAULT_INDEXER_URI`
    (`https://zec.rocks:443`) instead of an empty URI, since `GrpcIndexer::new`
    validates the scheme at construction.
  - `ZingoConfigBuilder`:
    - renamed: ClientConfigBuilder
    - reworked. public fields now private with public setter methods to constrain public API:
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
  - `new` constructor:
    - `network` parameter renamed `chain_type`
    - `wallet_base`, `birthday` and `wallet_settings` fields replaced by `wallet_config` field
  - new wallet serialization version 41 due to changes to chain type fmt::Display. chain type is now encoded as u8 and output indexes changed to u32.
  - `update_current_price` method no longer takes `tor_client` parameter.
- `wallet::keys::unified::UnifiedKeyStore`:
  - `new_from_seed` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
  - `new_from_mnemonic` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
  - `new_from_ufvk` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
- `wallet::disk`:
  - serialized version incremented to 41 for serializing output indexes as u32 and chain types as u8 instead of string.
  - `read` module: `network` parameter renamed `chain_type`
- `wallet::error::WalletError`: added `WalletAlreadyCreated` variant
- `wallet::error::KeyError`: added `InvalidMnemonicPhrase` variant
- `wallet::summary::data`:
  - `NoteSummary`: `output_index` field is now u32.
  - `OutgoingNoteSummary`: `output_index` field is now u32.
  - `CoinSummary`: `output_index` field is now u32.
  - `OutgoingCoinSummary`: `output_index` field is now u32.
- `wallet::output::OutputRef`: `output_index` method now returns u32.

### Removed
- `regtest` feature: production binaries can now be tested in regtest mode.
- `config` module:
  - `DEFAULT_LOGFILE_NAME` constant
  - `ZingoConfig`:
    - `logfile_name` method
    - `get_log_config` method
    - `get_log_path` method
    - `create_testnet` method
    - `create_mainnet` method
    - `create_unconnected` method
  - `ZingoConfigBuilder`:
    - `set_logfile_name` method
  - `ChainFromStingError`: replaced by `InvalidChainType` error struct.
  - `chain_from_str`: replaced by impl TryFrom<&str> for `ChainType`
  - `ZingoConfig`:
    - `get_wallet_with_name_pathbuf`
    - `get_wallet_with_name_path`
    - `wallet_with_name_path_exists`
    - `get_wallet_pathbuf`
    - `wallet_exists(`
  - `DEFAULT_LOGFILE_NAME` constant.
  - `ZingoConfig`:
    - `logfile_name` field
    - `logfile_name()` method
    - `get_log_config()` method
    - `get_log_path()` method
  - `ZingoConfigBuilder::set_logfile_name()` method.
  - `load_clientconfig`: replaced by zingo config builder pattern (`ZingoConfigBuilder`)
- `wallet::LightWallet`: `mnemonic` method.
- `testutils::lightclient::new_client_from_save_buffer`
- `wallet::WalletBase`: no longer public. public functionality replaced by `config::WalletConfig`
- `lightclient::LightClient`:
  - `create_from_wallet` constructor: no longer needed as now covered by `new` due to config rework
  - `create_from_wallet_path` constructor: no longer needed as now covered by `new` due to config rework
  - `tor_client` method. Tor no longer supported. To be replaced by nym in coming release.
  - `create_tor_client` method.
  - `remove_tor_client` method.
- `testutils::build_fvk_client`

## [4.0.0] - 2026-06-05

### Changed
- `lightclient::error::LightClientError`: added `SyncLaunchErrror` variant.
- `data::Receiver`: From impl for Payment is now a TryFrom

## [3.0.1] - 2026-03-26

## [3.0.0] - 2026-03-02

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
