# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- New `Calculated` variant to `ConfirmationStatus` enum for transactions that have been calculated but not yet broadcast
- Derive `Eq`, `PartialOrd`, and `Ord` traits for `ConfirmationStatus` to enable sorting
- Serialization and deserialization support with `read` and `write` methods using `byteorder`
- Additional confirmation checking methods:
  - `is_confirmed_after_or_at` to check if a transaction is confirmed at or after a specific block height
  - `is_calculated` to check if status is `Calculated`
  - `get_confirmed_height` to retrieve the height of a confirmed transaction

### Changed
- **Breaking**: Split `Pending` variant into two distinct states:
  - `Transmitted` for transactions sent to the blockchain
  - `Mempool` for transactions known to be in the mempool
- Reordered enum variants with `Confirmed` first, followed by `Mempool`, `Transmitted`, and `Calculated` (impacts ordering behavior)
- Display implementation now shows:
  - "Calculated for {height}" for `Calculated` status
  - "Transmitted for {height}" for `Transmitted` status
  - "Mempool for {height}" for `Mempool` status
  - "Confirmed at {height}" for `Confirmed` status
- `from_blockheight_and_pending_bool` now returns `Transmitted` instead of `Pending` for backward compatibility
- Updated documentation and doc tests throughout to reflect new status variants

### Deprecated
- `is_pending` method - use specific `is_transmitted` or `is_mempool` methods instead

### Removed
- Several redundant helper methods in favor of using `matches!` macro directly:
  - `is_transmitted` (use `matches!(status, ConfirmationStatus::Transmitted(_))`)
  - `is_mempool` (use `matches!(status, ConfirmationStatus::Mempool(_))`)

