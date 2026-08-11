# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
- `PriceSource::url` is public, so a Correspondable implementation can
  name the endpoint a source answers at.

### Changed

### Removed
- The dead sequential fetch path: `fetch_current_price`,
  `fetch_current_price_from`, `PriceList::update_current_price`, and
  `PriceSource::next`. Production moved to `race_current_price`, which races
  every source concurrently, and nothing called the sequential path anymore.
- The unreachable `PriceError::DecimalError` variant and the `rust_decimal`
  dependency that existed only to feed it. Price parsing has been float-based
  since the fetch re-implementation, so nothing could construct the variant.
  Its removal evicts twenty crates from the workspace lock file, among them
  the never-compiled `syn 1.0.109`.

## [0.1.0] - 2026-06-10

### Deprecated

### Added

### Changed
- `PriceError`: removed `TorError` variant.
- `PriceList`: `update_current_price` method no longer takes `tor_client` parameter. Tor no longer supported.

### Removed

## [0.0.1] - 2025-12-18
