# Claim: ZIP 318 convergence, deviations 3 and 4 (grilling session, 2026-07-23)

Session working in the `match_zip318` worktree on branch `match_zip318`.
The user ratified a minimal camouflage-first PR toward issue #2519: match
the reference boundary modulus (deviation 3) and adopt the canonical
rolling expiry (deviation 4). Every convergence edit carries a doc-comment
linking the most specific ZIP 318 GitHub anchor and, where one exists, a
line-anchored permalink into the librustzcash
`zcash_pool_migration_backend` implementation (commit `eb25d234`).

## Decisions ratified

- `bucket_modulus`: 256 → 144, matching the ZIP's provisional network-wide
  modulus M (equal to the reference `MEAN_DELAY`).
- `expiry_delta` is replaced in place by `expiry_modulus = 34_560`; the
  expiry window is derived as `2 * expiry_modulus`. Expiry becomes the
  canonical rolling function of the part's scheduled broadcast height.
- `MigrationParams::version` bumps 0 → 1; the store `INNER_VERSION` stays
  at 2 because the byte layout is unchanged (one u32 in the same slot).
  Dev wallets holding old state are caught by `params_hash` divergence.
- The age-0 anchor residue (our anchors are always the most recent
  boundary, which reference wallets never use) stays out of this PR; a
  draft comment for #2519 is handed to the user instead.

## File claims

- `zingolib/src/wallet/migration/params.rs`
- `zingolib/src/wallet/migration/store.rs` (field rename only)
- `zingolib/src/wallet/migration/schedule.rs` (canonical expiry helper)
- `zingolib/src/lightclient/migrate.rs` (the expiry computation site and
  any tests pinning the old constants)
- `zingolib/src/wallet/migration/reconcile.rs` (test module only: fixtures
  that hard-coded the old 256-block bucket geometry)
- `zingolib/src/wallet/migration/quantize.rs` (docs and tests: ZIP worked
  examples as vectors; ratified 2026-07-23 with the denomination-bounds
  convergence below)
- `zingolib/src/wallet/migration/split.rs` (test module only: fixtures
  built on the retired 0.001 ZEC denomination and 100 ZEC cap)

## Ratified follow-on (2026-07-23): denomination bounds + mirrored vectors

The survey for upstream test data exposed a further deviation: the ZIP
fixes the denomination set as {1, 2, 5} × 10^k with MAX_RESIDUAL_VALUE
= 0.01 ZEC and a largest crossing denomination of 10000 ZEC, while our
provisional set was powers of ten only, 0.001 through 100 ZEC. The user
ratified converging the set in this same PR (no new dependency; upstream
values copied with citations). Greedy decomposition over the full ladder
reproduces the ZIP's decimal digit expansion, so `decompose` itself is
unchanged. Mirrored test data: the ZIP's amount-selection worked
examples and digit table in quantize.rs, and the reference
`most_recent_boundary` goldens plus `expiry_examples` edges in
schedule.rs.

The user runs nextest/container suites; this session verifies with
`cargo check`, `cargo clippy`, `cargo fmt`, and fast targeted unit tests
only, and proposes a commit without creating one.

## Ratified follow-on (2026-07-23): deviation 5, k_max removal

The user ratified retiring deviation 5 on this same branch (PR #2520):
ZIP 318 places no cap on per-wallet multiplicity, so `k_max` is removed
from `MigrationParams` (store `INNER_VERSION` bumps to 3; a v2 read
discards the legacy slot), the `plan_schedule` clamp falls away, and the
whole cadence surface that existed only to set it — the `per_bucket`
argument of `start_ironwood_migration`, `reschedule_parts`, the
`CadenceFixed` error, and the zingo-cli `migrate cadence` subcommand —
is deleted. Additional file claim: `zingo-cli/src/commands.rs`.
Deviation 2 (anchor age, rolling witness cache) remains ratified but
unstarted; its design notes live in the session record.

## Done (2026-07-23)

All edits applied and verified: `cargo check` and `cargo clippy
--all-targets` clean, the full `cargo nextest run -p zingolib --lib`
suite green (251 passed), and the touched hunks rustfmt-clean (the
pre-existing drift at `migrate.rs:663` and `reconcile.rs:165` was left
alone). A new test pins the ZIP's worked expiry example; fixtures that
hard-coded the 256-block geometry now derive from the provisional
params. The commit itself is the user's to create.
