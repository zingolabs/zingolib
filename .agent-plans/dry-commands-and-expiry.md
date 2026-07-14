# Claim: commands DRY + expiry retarget (grilling session, 2026-07-13)

Session working in the `dry-commands` worktree on branch
`dry_commands_run_core` (stacked on PR #2458).

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

## File claims

- `zingolib/CONTEXT.md` (glossary updates as terms resolve)
- `docs/adr/` (possible ADR for the expiry mechanism)
- `zingo-cli/src/commands.rs`, `zingo-cli/src/commands/`,
  `zingo-cli/src/lib.rs`, `zingo-cli/src/tests.rs` (DRY layers, later)
- `zingolib/src/lightclient/offline.rs`, `zingolib/src/wallet/send.rs`
  (retarget mechanism, later)
