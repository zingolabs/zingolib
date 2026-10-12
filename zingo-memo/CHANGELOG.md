# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated
- `create_wallet_internal_memo_version_1`. The memo version follows the
  addresses: `create_wallet_internal_memo` writes version 1 for every list the
  deprecated function accepts and version 2 for the lists it refuses.

### Added
- Memo version 2, which records each unified address with its ZIP 316
  revision and every item it carries, metadata included, so an address read
  back on rescan is the one that was paid, and records no refund address
  indexes: `ParsedMemo::Version2`, `write_unified_address_with_revision` and
  `read_unified_address_with_revision`.
- `create_wallet_internal_memo`, which writes version 1 when every address is
  a Revision 0 address and version 2 otherwise, so a memo stays readable by
  releases that predate version 2 wherever version 1 suffices. It returns a `PackedMemo`: the
  memo field and how many of the addresses it records, since addresses that
  outgrow the field are left off the end rather than failing the memo.
- `MEMO_LEN`, the length of the memo field.
- `ParsedMemo::into_unified_addresses`, the addresses of a memo of any version.

### Changed
- `create_wallet_internal_memo_version_1` and `write_unified_address_to_raw_encoding`
  refuse an address that only ZIP 316 Revision 2 can represent, one with expiry
  metadata or without a shielded receiver, instead of recording it without its
  metadata or writing bytes the reader rejects.
- BREAKING: nothing writes refund address indexes any more.
  `create_wallet_internal_memo_version_1` takes the addresses alone and writes
  the refund address list its format frames as empty; `parse_zingo_memo` still
  reads the list from the version 1 memos of older releases.
- BREAKING: moves to `zcash_address` 0.14.0-pre.1, `zcash_keys` 0.17.0-pre.1,
  `zcash_encoding` 0.5, `zcash_protocol` 0.11.0-pre.0 and `rand` 0.10. The
  memo's raw unified address encoding is unchanged: it carries data items
  alone, read back as a ZIP 316 Revision 0 container.

### Removed
- `create_wallet_internal_memo_version_0`, deprecated since version 1 arrived.
  Version 0 memos are still read.

## [0.1.1] - 2026-06-05

## [0.1.0] - 2026-01-09

### Added

### Changed

- `create_wallet_internal_memo_version_0`: added `consensus_parameters` parameter
- `create_wallet_internal_memo_version_1`: added `consensus_parameters` parameter
- `write_unified_address_to_raw_encoding`: added `consensus_parameters` parameter

### Removed

## [0.0.1] - 2025-05-23
