# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

- `lightclient::LightClient::do_list_transactions`

### Added

- `lightclient::LightClient::save`
  - `export_save_buffer_runtime` fn
  - `get_wallet_file_location` fn
  - `get_wallet_dir_location` fn
- `wallet::keys::is_transparent_address` fn
- `wallet::notes::NoteRecordIdentifier` struct
- `utils` mod

### Changed

- `wallet::keys::is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- `wallet::transaction_record_map::TransactionRecordMap` -> `wallet::transaction_records_by_id::TransactionRecordsById`

### Removed

- `load_clientconfig` moved to zingoconfig crate
- `lightclient::LightClient`
  - `do_save`
  - `do_save_to_buffer`
  - `do_save_to_buffer_sync`
  - `fix_spent_at_height`
