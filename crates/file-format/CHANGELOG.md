# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- The crate holds the zingo wallet file: its content as one data type, and
  the readers and the writer of every layout version. It was split out of
  `zingolib`, whose `LightWallet` converts to and from it.
- `WalletFile<M>` holds the content of a wallet file as read. Only the
  readers of the crate build one. It has a read accessor named after each
  field, and `WalletFile::into_parts` moves the content out as a
  `WalletFileParts<M>`, whose fields are public. `WalletFileRef<'a, M>`
  borrows the same fields for writing. `WalletFile::read` reads a file under a given `ChainType`,
  `WalletFile::validate` checks the content, and `WalletFileRef::write`
  writes the current layout. `serialized_version` returns the version that
  build writes, `read_chain` reads the chain a file was written for, and
  `read_recovery_info` reads the seed and the birthday from the prefix of a
  version 32 or later file without parsing the rest.
- `MigrationSection` trait, which a consumer implements to read and write
  the migration section that follows the price list. `RawMigration` is an
  implementation that keeps the remaining bytes of the file unread and
  writes them back. Version 42 exists in a canonical layout and a
  pre-release layout. The reader parses the tail of a version 42 file in
  both layouts through every implementation, and it reads the file only
  when exactly one layout parses. It refuses a file whose tail parses in
  both layouts, because the bytes do not say which build wrote it.
  `RawMigration` accepts any bytes, so it refuses more version 42 files as
  ambiguous than an implementation that validates its section.
- `AmbiguousV42Tail`, the error that `WalletFile::read` returns inside its
  `io::Error` for a version 42 file whose tail parses in both layouts, so a
  caller can downcast to it. `WalletFile::read_with_v42_layout` reads such a
  file in the `V42Layout` the caller names, `Canonical` or `PreRelease`, and
  reads every other version as `WalletFile::read` does.
- `UnreadableV42Tail`, the error that `WalletFile::read` returns inside its
  `io::Error` for a version 42 file whose tail parses in neither layout. Its
  `canonical` and `pre_release` fields hold the error of each reading
  unchanged, so a caller can downcast to it and then to the cause of either
  reading. The `io::Error` keeps the kind of the canonical error, and the
  `source` of `UnreadableV42Tail` is the canonical error, so a walk of the
  source chain reaches the cause of the canonical reading. Its text ends with
  the text of the canonical error, so a walker that skips a source the text
  already ends with does not print that text twice.
- `legacy` feature, off by default. It adds the readers of the layouts below
  version 32 and the dependencies they need, among them `lightwallet-protocol`,
  `prost` and `zcash_client_backend`. A build without it returns from
  `WalletFile::read` and `read_chain` an `InvalidData` error that names the
  feature for a file below version 32, and `WalletFile::read`
  decodes versions 32 and above with no network-protocol dependency.
- `strings` module with `read_string` and a re-export of `write_string`.
- `WalletSettings` and `RecoveryInfo`, which `zingolib` re-exports at their
  old paths, `ChainMismatch`, and the `MAX_RECOVERABLE_VERSION` and
  `MAX_RECOVERABLE_BIRTHDAY` constants.

### Changed

### Deprecated

### Removed

### Fixed
