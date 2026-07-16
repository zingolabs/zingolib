# OP_RETURN via in-tree spend pipeline (zcb removal)

Worktree: `feat/opreturn_on_proposal`, branch `opreturn_on_proposal`.

Base: `zingolabs/feat/ironwood` (PR #2419, `b5a1b739e`) with PR
#2464's eight commits (`remove_stringly_typed_errors_from_zingo_cli`)
rebased on top at the user's direction — replayed conflict-free,
whole-workspace `cargo check --all-targets` green on the result. The
pipeline's error design extends #2464's typed regime: typed values
out, typed error enums per layer, JSON rendering only at the shell.

## Work declared

Scoping session (grill-with-docs) producing ADR 0010 and a plan for
reimplementing the minimum `zcash_client_backend` orchestration as a
zingolib module so OP_RETURN gets a native seam and zingolib depends
only on upstream crates.io releases of the zcash crates.

## Sequencing against PRs #2419 (ironwood) and #2464

#2419 lands ASAP; #2464 lands after it. #2464's remote branch is
still dev-based — the canonical rebase belongs to its owner, and this
branch's local replay must be re-aligned if that rebase lands
differently. This branch merges after both; code phases are unblocked
locally on the ironwood base.

Ironwood-era anchors, re-verified on this base: zcb pinned at
0.24.0-rc.1 (an RC — the release-cadence treadmill ADR 0010 ends);
`zcb_traits.rs` is 1,095 lines with 49 of 69 trait methods
`unimplemented!()`; the build choke point remains
`wallet/send.rs::create_proposed_transactions`;
`add_transparent_null_data_output` is present in zcash_primitives
0.29.0 / zcash_transparent 0.9.0, so the OP_RETURN foundation is
unaffected by the bumps. The P3 equivalence tests freeze zcb 0.24
(ironwood-era) behavior — the right baseline to preserve. #2419 owns
`docs/adr/0009`; ours is 0010.

## File claims

- `docs/adr/0010-*.md` (new)
- `zingolib/CONTEXT.md` (glossary updates as terms crystallise)
- Later, on implementation: `zingolib/src/wallet/propose.rs`,
  `zingolib/src/wallet/send.rs`, `zingolib/src/wallet/zcb_traits.rs`
  (delete), `zingolib/src/data/proposal.rs`, `zingolib/src/mocks.rs`,
  `zingolib/Cargo.toml`, new `zingolib/src/wallet/spend/`.

No other files are claimed; no commits without the user.

## Ratified decisions

All recorded in `docs/adr/0010-in-tree-spend-pipeline.md`; glossary term
"OP_RETURN Data" added to `zingolib/CONTEXT.md`.

1. `zcash_client_backend` leaves zingolib's manifest; `zip321` becomes a
   direct dependency; pepper-sync is out of scope.
2. The replacement is an in-tree module (`zingolib/src/wallet/spend/`);
   the four zcb trait impls are deleted, not re-abstracted.
3. Owned proposal types: enum `Transfer(Step)` /
   `TexTransfer { shielding, exposure }` / `Shield(Step)`; concrete,
   generic-free `Step` with `op_return_data: Option<OpReturnData>`
   validated by construction.
4. API: `OpReturnData` newtype (≤ 80 bytes, checked at proposal time);
   `Option<OpReturnData>` parameter on `propose_send` and
   `propose_send_all` only; never on `propose_shield`.
5. Functional layering: pure plan (`&LightWallet` → proposal),
   wallet-pure build (proposal + keys + witnesses + provers →
   transactions, randomness internal), single apply mutation site.
   Refund addresses derive at plan, reserve at apply. ADR 0006 slot and
   `pause_sync` untouched.
6. Errors extend PR #2464's typed regime (`MemoError::TooLong` is the
   template for `OpReturnDataError::TooLong`); one enum per layer,
   composing into the existing client error chain.
7. ZIP-317: fee arithmetic stays upstream; the plan layer feeds
   `FeeRule::fee_required` the serialized size of every transparent
   output it emits, the null-data `TxOut` included.

## Phases

Each phase is a reviewable unit on top of the ironwood-plus-#2464
base; the branch must not merge ahead of either PR.

- **P1 — trims.** DONE in working tree (uncommitted). Dropped the
  vestigial `lightwalletd-tonic` feature (workspace) and the equally
  vestigial `lightwalletd-tonic-transport` (zingo-testutils — file
  claim extended to `zingo-testutils/Cargo.toml`, unclaimed by other
  plans); `zip321 = "0.9.0-rc.1"` promoted to a direct workspace dep
  (already in the lockfile via zcb — no new crate); all 15 code and 4
  doc-link references rewritten from `zcash_client_backend::zip321` to
  `zip321`. Lockfile delta: zcb loses `tonic`, `tonic-prost`,
  `hyper-util`; zingolib gains `zip321`. Verified: workspace
  `cargo check`/`clippy --all-targets` clean, `cargo fmt` clean.
  Import-path/manifest-only — no behavior change; nextest left to the
  user (ironwood fast suite has 4 known-red, unrelated).
- **P2 — owned types.** DONE in working tree (uncommitted).
  `wallet/spend/` module: `Proposal` enum
  (`Transfer`/`TexTransfer`/`Shield` variant structs with account id and
  target height owned), concrete `Step` (request, payment pools,
  shielded/transparent inputs, change, fee, `Option<OpReturnData>`),
  `OpReturnData` (validated ≤ 80 bytes, `OpReturnDataError::TooLong`
  mirroring `MemoError`), and `ProposalShapeError` enforced by the
  proposal constructors (no OP_RETURN on Shield or on the TEX shielding
  step; no shielded inputs on Shield). `with_target_height` is the pure
  ADR 0008 retarget mechanism. 8 unit tests green; check/clippy/fmt
  clean. `ZingoProposal` folds into the new enum at P5 cutover, when
  the facade rewires.
- **P3 — plan layer (pure).** Selection (core loop exists in
  `select_spendable_notes`), single-output Orchard change + dust
  policy, ZIP-320 TEX splitting, refund-address derivation, fee sizing
  per decision 7. Equivalence tests against zcb's `propose_transfer` /
  `propose_shielding` on `SyntheticWalletBuilder` wallets: same
  request in, same inputs/change/fee out.
- **P4 — build layer (wallet-pure).** Witness extraction at the edge;
  `zcash_primitives::Builder` orchestration; TEX step 2 spends step 1's
  ephemeral output; `add_transparent_null_data_output` on the final
  step; USK signing, sender-OVK policy. ADR 0008 retarget becomes a
  pure field update.
- **P5 — apply + cutover.** Facade rewires to plan/build/apply;
  `propose_send`/`propose_send_all` gain the `Option<OpReturnData>`
  parameter; reserve-at-apply; delete `zcb_traits.rs`, prune
  `mocks.rs`, delete the equivalence scaffolding, remove zcb from
  `zingolib/Cargo.toml`; re-home `read_shard` (reimplement or reach it
  through pepper-sync's surface); fix the 3 zcb refs in
  libtonode-tests.
- **P6 — end-to-end.** Container/libtonode round-trip: a swap-shaped
  send whose final transaction carries OP_RETURN Data, verified on a
  regtest chain. User-run (`makers container-test`); agent proposes
  commands and pass/fail signals.

Estimated new pipeline code ~1.5–2.5k lines against the 1,095-line
trait file deleted (49 of its 69 methods are stubs) plus mock pruning. zingo-mobile FFI exposure of the new parameter
is zingo-mobile work, out of scope here.
