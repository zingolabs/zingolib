# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added

### Changed
- **Breaking.** A failing command now renders exactly once, as `Error: …` on
  stderr, and one-shot mode exits nonzero. Both the message and the failure
  itself previously went to stdout with an exit code of 0, so a failed send was
  indistinguishable from a successful one except by parsing prose (ADR 0031).
- **Breaking.** Stdout now carries exactly one thing, the command's result. The
  in-band `{"error": …}` JSON objects that used to appear inside otherwise
  successful output are gone, and their content travels as a typed command
  error on stderr. A consumer that parses `{"error"}` out of stdout must read
  stderr and the exit code instead (ADR 0031).
- **Breaking.** An unknown command is now an error rather than a line of help
  text on stdout, so a mistyped command fails loudly instead of appearing to
  succeed (ADR 0031).
- Progress narration moved from stdout to stderr: the transmit heartbeat, the
  indexer probing notices, the save and sync launch notices, and the trailer
  that `quit` prints while the wallet saves. Nothing may parse stderr
  (ADR 0031).
- Command dispatch now runs from a static command table of async command
  bodies, and the crate crosses from sync to async at a single audited seam
  that a Clippy `disallowed-methods` rule enforces. This is an internal change
  that alters no command's output (ADR 0030).

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

