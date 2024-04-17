# Changelog

All notable changes to this crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

- `lightclient::LightClient::do_list_transactions`

### Added

- `lightclient::LightClient`:
  - `export_save_buffer_runtime`
  - `get_wallet_file_location`
  - `get_wallet_dir_location`
- `wallet::keys::is_transparent_address` fn
- `wallet::notes::NoteRecordIdentifier` struct
- `utils` mod
- `lightclient::LightClient`:
  - `do_propose` behind "zip317" feature
  - `do_send_proposal` behind "zip317" feature
- `commands`:
  - `ProposeCommand` struct and methods behind "zip317" feature
  - `QuickSendCommand` struct and methods behind "zip317" feature
- `data::proposal` mod

- `test_framework` mod behind "test-features" feature

### Changed

- `wallet::keys::is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- `wallet::transaction_record_map::TransactionRecordMap` -> `wallet::transaction_records_by_id::TransactionRecordsById`
- `commands`:
  - `get_commands` added propose and quicksend to entries behind "zip317" feature
  - `SendCommand::help` formatting

### Removed

- `load_clientconfig` moved to zingoconfig crate
- `lightclient::LightClient`:
  - `do_save`
  - `do_save_to_buffer`
  - `do_save_to_buffer_sync`
  - `fix_spent_at_height`
