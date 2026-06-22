# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
- Optional positional `op_return` argument on the `send` and `quicksend` commands.
  Accepts a hex-encoded payload (max 80 bytes raw / 160 hex chars) attached to the
  transaction as a transparent `OP_RETURN` null-data output. Used by cross-chain
  swap integrations (THORChain/MAYAChain) that require a swap memo embedded in the
  transaction's null-data output.
  - Positional form: `quicksend <address> <amount> "<memo>" "<op_return_hex>"`
    (use an empty memo `""` when only `op_return` is needed).
  - JSON form: `"op_return"` field accepted on the first receiver entry.
- `hex` workspace dependency for decoding the new `op_return` argument.
- `CommandError::InvalidOpReturn` variant.

### Changed

### Removed

## [0.4.0] - 2026-06-10

### Removed
- `regtest` feature: can still use zingo-cli in regtest mode with no features enabled using the '--chain regtest' flag. 
- `tor` flag. tor is no longer supported but will be replaced by nym in the coming release.

## [0.3.0] - 2026-06-05

### Changed
`remove_transaction` command - now only allows transactions with the new `Failed` status to be removed.

### Removed
- `resend` command: see zingolib CHANGELOG.md on `LightClient::resend`
- `send_progress` command

## [0.2.0]

