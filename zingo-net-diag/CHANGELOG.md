# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

## [0.1.0] - 2026-10-15

### Added
- First release. The shared network-failure taxonomy: `NetOpStage` names the
  stage of a covered network operation that failed, `NetOpFailure` records the
  stage, the target and the full cause chain as one text per layer, and
  `chain_texts` walks an error's source chain. The crate is std-only with no
  dependencies, with `serde` support behind the optional `serde` feature.
