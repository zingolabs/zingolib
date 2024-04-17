# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

- `lightclient::LightClient::do_list_transactions`

### Added

<<<<<<< HEAD
- GrpcConnector::get_client --> pub
- LightClient pub fn export_save_buffer_runtime
- LightClient pub fn get_wallet_file_location
- LightClient pub fn get_wallet_dir_location
- `wallet::keys`:
  - `is_transparent_address`
- `lightclient::send`
  - `do_propose` behind "zip317" feature
  - `do_send_proposal` behind "zip317" feature
- `commands`
  - `ProposeCommand` struct and methods behind "zip317" feature
  - `QuickSendCommand` struct and methods behind "zip317" feature
- pub struct crate::wallet::notes::NoteRecordIdentifier
||||||| ba38d202
- GrpcConnector::get_client --> pub
- LightClient pub fn export_save_buffer_runtime
- LightClient pub fn get_wallet_file_location
- LightClient pub fn get_wallet_dir_location
- `wallet::keys`:
  - `is_transparent_address`
- pub struct crate::wallet::notes::NoteRecordIdentifier
=======
- `lightclient::LightClient`:
  - `export_save_buffer_runtime`
  - `get_wallet_file_location`
  - `get_wallet_dir_location`
- `wallet::keys::is_transparent_address` fn
- `wallet::notes::NoteRecordIdentifier` struct
- `utils` mod
- `utils::txid_from_hex_encoded_str` fn
- `lightclient::LightClient`:
  - `do_propose` behind "zip317" feature
  - `do_send_proposal` behind "zip317" feature
- `commands`:
  - `ProposeCommand` struct and methods behind "zip317" feature
  - `QuickSendCommand` struct and methods behind "zip317" feature
- `test_framework` mod
>>>>>>> labs/dev

### Changed

<<<<<<< HEAD
- load_client_config fn moves from zingolib to zingoconfig
- `wallet::keys`:
  - `is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- `commands`
  - `get_commands` added propose and quicksend to entries behind "zip317" feature
  - `SendCommand::help` formatting
- zingolib/src/wallet/transaction_record_map.rs -> zingolib/src/wallet/transaction_records_by_id.rs
- TransactionRecordMap -> TransactionRecordsById
||||||| ba38d202
- load_client_config fn moves from zingolib to zingoconfig
- `wallet::keys`:
  - `is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- zingolib/src/wallet/transaction_record_map.rs -> zingolib/src/wallet/transaction_records_by_id.rs
- TransactionRecordMap -> TransactionRecordsById
=======
- `wallet::keys::is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- `wallet::transaction_record_map::TransactionRecordMap` -> `wallet::transaction_records_by_id::TransactionRecordsById`
- `commands`:
  - `get_commands` added propose and quicksend to entries behind "zip317" feature
  - `SendCommand::help` formatting
>>>>>>> labs/dev

### Removed

- `load_clientconfig` moved to zingoconfig crate
- `lightclient::LightClient`:
  - `do_save`
  - `do_save_to_buffer`
  - `do_save_to_buffer_sync`
  - `fix_spent_at_height`
