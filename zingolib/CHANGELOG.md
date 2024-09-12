# Changelog

All notable changes to this crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `TxMapAndMaybeTrees` renamed `TxMap`

### Removed

- `lightclient.bsync_data.uri()`

## [mobile-release-1.4.3-0-g9fa99407]

### Deprecated

- `lightclient::LightClient::do_list_transactions`
- `wallet::keys::is_shielded_address`

### Added

- `lightclient::LightClient`:
  - `export_save_buffer_runtime`
  - `get_wallet_file_location`
  - `get_wallet_dir_location`
  - `propose_test_only` behind `test-features` flag
  - `send_test_only` behind `test-features` flag
  - `shield_test_only` behind `test-features` flag
  - `transaction_request_from_send_inputs` behind `test-features` flag
- `wallet::notes::NoteRecordIdentifier` struct
- `utils` mod
- `lightclient::LightClient`:
  - `propose_send` behind "zip317" feature
  - `propose_send_all` behind "zip317" feature
  - `propose_shield` behind "zip317" feature
  - `send_proposal` behind "zip317" feature
- `commands::get_commands`:
  - `propose` to `entries` behind "zip317" feature
  - `proposeall` to `entries` behind "zip317" feature
  - `quicksend` to `entries` behind "zip317" feature
- `data::proposal` mod
- `test_framework` mod behind "test-features" feature

### Changed

- `wallet::keys::is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- `wallet::transaction_record_map::TransactionRecordMap` -> `wallet::transaction_records_by_id::TransactionRecordsById`
- `commands`:
  - `get_commands` added sendall, quicksend and quickshield to entries behind "zip317" feature
  - `SendCommand::help` formatting
- `lightclient::LightClient`:
  - `do_send` inputs from `Vec<(&str, u64, Option<MemoBytes>)>` to `Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>`
  - `do_shield` inputs from `Option<String>` to `Option<Address>`
  - `do_list_txsummaries` --> `list_value_transfers`

- `TxMapAndMaybeTrees::A --> TransactionRecordsById::A` where A:
  - add_new_note<D>
  - check_notes_mark_change
  - add_taddr_spent
  - total_funds_spent_in
  - set_price

### Removed

- `test-features` feature gate
- `load_clientconfig` moved to zingoconfig crate
- `lightclient::LightClient`:
  - `do_save`
  - `do_send`
  - `do_shield`
  - `do_save_to_buffer`
  - `do_save_to_buffer_sync`
  - `fix_spent_at_height`
  - `TransactionRecord::net_spent`
  - `TransactionRecord::get_transparent_value_spent()`
- `LightWallet`:
  - `send_to_addresses`
