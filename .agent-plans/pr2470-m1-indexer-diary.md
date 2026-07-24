# PR #2470 review M1: gate, opt-in, cap, and sanitize the indexer history

CLAIMED 2026-07-23 by the review-remediation session. Ratified by the
user in-session: the indexer history ("diary") must be (1) compile-gated
behind a dedicated `nym-diary` feature, (2) opt-in at runtime even when
compiled, (3) capped so the file cannot grow without bound, and
(4) sanitized so no raw server prose (which can embed txids) reaches
disk.

## File claims

- `zingolib/Cargo.toml` — the `nym-diary` feature.
- `zingolib/src/lightclient/indexer_history.rs` — gating, runtime
  enablement, cap, sanitization, and their tests.
- `zingolib/src/lightclient.rs` — constructor wiring and the runtime
  opt-in surface.
- `zingolib/src/lightclient/send.rs` — the recording call sites (typed
  outcome instead of raw strings).
- `zingolib/src/lightclient/transmit.rs` — reuse of the rejection
  classifier for sanitization, if extraction is needed.
- `zingolib/src/nym/probe.rs` — the probe recording call sites.
- `zingo-cli/Cargo.toml`, `zingo-cli/src/lib.rs`,
  `zingo-cli/src/commands.rs` — feature forwarding, the runtime opt-in
  flag, and whatever the history renderer needs.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make. The
shape that landed: a `nym-diary` feature in zingolib (forwarded from
zingo-cli), an inert-by-default `IndexerHistoryHandle` whose disk-backed
form only the diary build constructs, a shared `recording` switch that
`LightClient::set_indexer_diary` flips per session (the CLI's
`--indexer-diary` flag drives it, and a diary-less build warns loudly),
a closed `FailureKind` token set replacing raw failure prose on disk
(legacy prose lines re-classify on load), and compaction to the newest
1024 records once the file doubles that. `nym history` renders in the
diary build and names the missing feature otherwise. CI's nym-feature
job clippies `nym,nym-diary` and runs the diary tests.

Verified: clippy clean on default, `nym`, `nym-diary`, and
`nym,nym-diary` (zingolib + zingo-cli, all targets); nextest green on
the 9 diary tests (default build), 37 nym+diary zingolib tests, and 10
zingo-cli nym/history tests.
