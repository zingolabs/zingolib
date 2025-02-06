# Changelog

All notable changes to this crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.9.0]

### Added

- `commands`:
  - `MessagesFilterCommand`: For listing `ValueTransfers` containing memos related to an optional `sender` parameter. Returns a `JsonValue` object.
  - `ValueTransfersCommand`: takes optional reverse sort argument
  - `lightclient::messages_containing`: used by `MessagesFilterCommand`.
- `lightclient::sorted_value_transfers`
- `tests`:
  - `message_thread` test.

### Changed

- `LightClient::value_transfers::create_send_value_transfers` (in-function definition) -> `ValueTransfers::create_send_value_transfers`

### Removed

- `lightclient.do_list_notes` is deprecated
