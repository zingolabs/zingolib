# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- BREAKING: `get_source_price` takes a `zingo_netutils::conduit::ConduitDial`
  where it took `Option<&str>`. A price fetch is mixnet-only (ADR 0011), and
  the option let a caller passing `None` route it over clearnet instead. The
  clearnet leg survives crate-privately for this crate's own tests, where no
  caller outside can reach it, so the public API can no longer express the
  fetch the ADR forbids. The `socks5-fetch` feature gains a `zingo-netutils`
  dependency to carry the guard type.

### Deprecated

### Added
- `first_quote` turns a driven race's outcomes into the first quote, or the
  report naming every source's typed failure when none answered.
- `RACED_SOURCES`, the price census a run races, so a caller driving the
  race asks the same operators in the same order.
- `get_source_price` is public: one source's fetch, which a caller composes
  into a race of its own.

### Removed
- BREAKING: `race_current_price` and the private `race_sources` are gone.
  The wallet runs one speed-priority wave for every operation that races
  targets through an Exit Node — the Server-Selection Sweep and the price
  run alike — so this crate no longer carries a second racing mechanism of
  its own. It says what to fetch; the wallet says how to race it. A caller
  that raced through this crate now races `RACED_SOURCES` with
  `get_source_price` and collects the outcomes with `first_quote`, or, in
  the wallet, runs `zingolib::mixnet::speed::run_wave` over `PriceRun`.

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
