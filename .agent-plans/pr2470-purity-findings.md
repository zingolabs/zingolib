# Claim: PR #2470 purity-review findings 1–7 applied test-first (2026-07-20)

A review of PR #2470 (the reboot_nym branch) identified seven places where
logic can be expressed as pure, side-effectless functions, several taking
functions as arguments. The user directed that findings 3, 4, 1, 2, 5, 6,
and 7 be applied in this worktree, each preceded by a unit test that fails
because of the issue, or by documentation of why no such test can exist.

All edits are additive seams (extract-pure-core, inject-collaborator) so
they compose with the nym implementation work claimed in
`nym-transmission.md`. The `zingo-cli/tests/log_file.rs` modification in
the tree belongs to that session and is not touched here.

## File claims

- `zingo-netutils/src/nym_proxy.rs` — findings 3 (strip_socks5_scheme),
  1 (retry engine extraction), 2 (provider extraction + seeded shuffle).
- `zingo-netutils/src/mixnet_connect.rs` (new) — the ungated pure retry
  engine and seeded shuffle, with their unit tests.
- `zingo-netutils/src/lib.rs` — finding 4 (shared `parse_send_response`,
  fixing the lone-quote panic in `GrpcIndexer::send_transaction`).
- `zingo-netutils/src/socks5_transmit.rs` — finding 4 (reuse the shared
  parser).
- `zingo-cli/src/commands.rs` — findings 5 (`choose_proxy_path` pure
  precedence core) and 7 (`render_status` pure renderer).
- `zingolib/src/lightclient/transmit.rs` — finding 6 (`classify_rejection`
  pure classifier).

## Status

APPLIED (2026-07-20), uncommitted, all verification green. Finding 4
followed strict red-green: the verbatim-extracted `parse_send_response`
panicked on a lone-quote acceptance (`begin <= end (1 <= 0)`), proving the
latent clearnet panic; the length guard fixed it and both callers now
share the parser. The other findings are behavior-preserving extractions
for which no failing test is producible — their pure cores are the
enabling refactor, and each now carries direct unit tests (12 new in
zingo-netutils' default build, 2 in zingolib, 3 in zingo-cli).

Verified: netutils default `cargo test` (22 pass), netutils
`--features nym` check and `--all-features` clippy, main-workspace check
and clippy `-D warnings` with and without `--features nym`, zingolib
transmit tests (12 pass) and `nym::` tests (18 pass), zingo-cli
`nym_command_parsing` (6 pass). Commits are the user's to make.

Note for the implementing session: `seeded_shuffle` keeps the LCG (the
no-new-deps rule kept `rand` out of netutils); if nym-sdk's re-exports
ever surface an `Rng`, swapping the seed injection for RNG injection is
a drop-in.
