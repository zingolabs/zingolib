# Claim: independent verification of the #2419 review findings (2026-07-16)

Session working in the `feat_ironwood` worktree on branch `feat_ironwood`.
The user handed over six correctness findings from a reviewer of PR #2419
and asked for an independent review plus an attempted failing test that
demonstrates each finding. The deliverable is test code and a verdict per
finding, not fixes. Any product-source change waits for the user's
ratification.

## Scope

The work began as tests only; the user then ratified fixing each confirmed
finding, with test and fix combined per commit on the `six_edges_tested`
branch (PR against feat/ironwood, Oscar reviewing). Verdicts: findings 1,
3, 4, 6a, 6b, and 6c confirmed and fixed; finding 2 resolved in the code's
favor (the comment was stale) and pinned by test; finding 5 refuted and
pinned by test. Fixtures added while DRYing the tests:
`testutils::synthetic_wallet::inject_confirmed_orchard_notes`, the
migrate.rs test-module scaffold helpers, and `planned_state` in the store
tests.

## File claims (test-only additions)

- `zingolib/src/wallet/zcb_traits.rs` (findings 1 and 2: AllFunds panic,
  ironwood OutputRef pool label)
- `zingolib/src/lightclient/migrate.rs` (finding 3: paused sync leak on the
  note-split round)
- `pepper-sync/src/sync/state.rs` (finding 4: chain-tip range fallback with
  empty ironwood shard ranges)
- `zingolib/src/wallet/migration/schedule.rs` and
  `zingolib/src/wallet/migration/reconcile.rs` (finding 5: open-window part
  at relaunch; finding 6c: unknown txid treated as failed)
- `zingolib/src/wallet/migration/store.rs` (finding 6a: u64 to usize
  truncation)
- `pepper-sync/src/scan/compact_blocks.rs` (finding 6b: lenient ironwood
  tree-size check)
- Possibly a new test file under `zingolib/tests/` or
  `pepper-sync/tests/` if an inline `mod tests` does not fit.

No behavior changes to product source. Long test suites stay with the user,
this session runs only targeted fast tests through `cargo nextest run`.
