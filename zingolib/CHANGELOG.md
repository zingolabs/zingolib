# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

- LightClient::do_list_transactions

### Added

- GrpcConnector::get_client --> pub
- LightClient pub fn export_save_buffer_runtime
- LightClient pub fn get_wallet_file_location
- LightClient pub fn get_wallet_dir_location
- `wallet::keys::is_transparent_address`
- pub struct crate::wallet::notes::NoteRecordIdentifier
- `utils` mod
- `utils::txid_from_hex_encoded_str` fn
- `test_framework` module
- `test_framework::mocks` module
- `lightclient::send`
  - `do_propose` behind "zip317" feature
  - `do_send_proposal` behind "zip317" feature
- `commands`
  - `ProposeCommand` struct and methods behind "zip317" feature
  - `QuickSendCommand` struct and methods behind "zip317" feature

### Changed

- load_client_config fn moves from zingolib to zingoconfig
- `wallet::keys::is_shielded_address` takes a `&ChainType` instead of a `&ZingoConfig`
- zingolib/src/wallet/transaction_record_map.rs -> zingolib/src/wallet/transaction_records_by_id.rs
- TransactionRecordMap -> TransactionRecordsById
- `commands`
  - `get_commands` added propose and quicksend to entries behind "zip317" feature
  - `SendCommand::help` formatting

### Removed

- LightClient pub fn do_save
- LightClient pub fn do_save_to_buffer
- LightClient pub fn do_save_to_buffer_sync
- LightWallet pub fn fix_spent_at_height
