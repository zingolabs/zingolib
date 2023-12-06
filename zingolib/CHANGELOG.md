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

### Changed
- load_client_config fn moves from zingolib to zingoconfig

### Removed

- LightClient pub fn do_save
- LightClient pub fn do_save_to_buffer
- LightClient pub fn do_save_to_buffer_sync
- LightWallet pub fn fix_spent_at_height
