# Claim: commands DRY + expiry retarget (grilling session, 2026-07-13)

Session working in the `dry-commands` worktree on branch
`remove_stringly_typed_errors_from_zingo_cli` (rebased onto dev
2026-07-14; formerly `dry_commands_run_core` stacked on PR #2458, which
dev's retarget work superseded). Live PR: #2464.

## Decisions ratified so far

- No new dependencies and no `[patch]`/fork pins, ever. The #2458 fork
  sentinel and the PCZT route are dead; expiry goes via proposal
  retargeting through public zcb 0.23 APIs.
- Retarget policy: max out under both caps —
  `min(ZIP203 max, next scheduled activation − 1)`, target derived by
  subtracting the upstream `DEFAULT_TX_EXPIRY_DELTA`. Verified end to
  end: zebra has no expiry-distance rule and zaino/lwd are passthroughs.
- `calculate` reports `expiry_height` as a JSON number; the "no
  practical expiry, valid until next NU" semantics live in help text.
  Step 0 goldens assert that field structurally, not byte-for-byte.
- Local const `ZIP203_TX_EXPIRY_HEIGHT_THRESHOLD = 500_000_000` (max
  derived from it); upstream issue filed asking zcash_protocol to
  export a named constant, to be adopted at a normal version bump.

## In progress (2026-07-14)

- Eliminate the REPL's in-band error sniffing in `zingo-cli/src/lib.rs`
  (the internal instance of issue #2446's problem; same family as the
  Least Authority audit's Issue Q, which itself concerns zingo-mobile's
  FFI bridges). The prompt indicator and the save check move onto the
  command-loop thread and use typed calls (`poll_sync`,
  `pepper_sync::sync_status`, `check_save_error`); the channel request
  becomes an enum so no consumer classifies a response by its content.
  All printed bytes are preserved, including the dependence removal on
  pepper-sync's misspelled success message — except the sync progress
  itself, which by user direction is now an exact integer ratio
  (`X / Y outputs`), never a float. To support that,
  `pepper-sync/src/sync.rs` (claimed) exposes the ratio's integers on
  `SyncStatus` (`total_outputs_scanned` / `total_outputs`, both `u64`)
  and an `is_complete()` that preserves the refetching-nullifiers
  nuance the old 99% override encoded.
- Branch rebased onto zingolabs/dev (2026-07-14): the fork-based stack
  base b84459994 and the local ADR 0007 were dropped as superseded by
  dev's retarget implementation (c007996aa) and ADR 0008; the ADR 0006
  pointer fix and this file were replayed as the rebase remnant.

## File claims

- `zingolib/CONTEXT.md` (glossary updates as terms resolve)
- `docs/adr/` (possible ADR for the expiry mechanism)
- `zingo-cli/src/commands.rs`, `zingo-cli/src/commands/`,
  `zingo-cli/src/lib.rs`, `zingo-cli/src/tests.rs` (DRY layers, later)
- `zingolib/src/lightclient/offline.rs`, `zingolib/src/wallet/send.rs`
  (retarget mechanism, later)
