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

## In progress (2026-07-14, #2446 close-out; DRY deferred)

Executing the three ratified steps on PR #2464 (audit evidence in
issue #2465): (1) `do_delete` error typed in place (`io::Error`);
(2) `memo_bytes_from_string` error typed in place (`MemoError`);
(3) `info()` returning a typed `ServerInfo` (JsonValue conversion, not
serde — no new deps). The user then ratified crossing the API boundary:
`do_info` is DELETED, not wrapped — mobile and pc adapt at their next
pin bump. Additional claims:
`zingolib/src/lightclient.rs`, `zingolib/src/lightclient/save.rs`,
`zingolib/src/wallet/utils.rs`, `zingo-cli/src/commands/utils.rs`.

## In progress (2026-07-14, review follow-up on PR #2464)

The PR review found two defects in the new `SyncStatus::is_complete`:
a false negative when the birthday-to-chain-height range contains no
shielded outputs (the `total_outputs > 0` guard conflated "never
started" with "nothing to scan"), and an inherited false positive when
chain growth pushes `total_outputs_scanned` past a stale
`total_outputs` while ranges remain unscanned. Both are fixed by
redefining completion as the sync task's own terminal condition: sync
has started (`sync_start_height != 0`, the existing sentinel) and every
scan range has priority `Scanned`. The duplicated scan-priority
predicates gain named helpers on `ScanPriority` (`is_scanned`,
`awaits_nullifier_retrieval`) used across `pepper-sync/src/sync.rs` and
`pepper-sync/src/sync/state.rs` (now also claimed). Unit tests pin the
completion contract in pepper-sync, where it is defined.

## File claims

- `zingolib/CONTEXT.md` (glossary updates as terms resolve)
- `docs/adr/` (possible ADR for the expiry mechanism)
- `zingo-cli/src/commands.rs`, `zingo-cli/src/commands/`,
  `zingo-cli/src/lib.rs`, `zingo-cli/src/tests.rs` (DRY layers, later)
- `zingolib/src/lightclient/offline.rs`, `zingolib/src/wallet/send.rs`
  (retarget mechanism, later)
- `pepper-sync/src/sync.rs`, `pepper-sync/src/sync/state.rs`
  (`is_complete` fix and scan-priority helpers)
