# Claim: view-model crate (grilling session, 2026-07-20)

**STATUS: GRILLING — no design ratified, no code edits yet.**

Goal: implement a "view-model" crate per dorianvp's reductions analysis
(hackmd HJ0jWQh4zx) — move the editorial/presentation reductions out of
zingolib's summary layer into a consumer-facing crate, with the minimum
change required of zingo-mobile. Work rides the mixnet branch
(`view-model`, stacked on the nym increments in this worktree).

Branch: **`viewmodel-crate`**, cut 2026-07-20 from the `view-model`
tip (c66553fdd "fix: repair the reboot_nym rebase fallout"), so the
work rides the full mixnet arc. Home: worktree
**`/home/nattyb/src/zingolabs/zls/viewmodel`** (created 2026-07-20 at
the user's direction so `reboot_nym` stays free for the nym-specific
work; this session works ONLY here).

## File claims (prospective, gated on ratification)

- `.agent-plans/view-model.md` (this file)
- `zingo-viewmodel/` (or whatever name is ratified) — new crate.
- `zingolib/src/wallet/summary*` — stripping editorial reductions,
  once the split is ratified.
- `zingolib/CONTEXT.md` — glossary entries as terms resolve.
- `docs/adr/` — ADR if the split proves ADR-worthy.

## Decisions ratified

1. **Charter = (a)+mixnet (2026-07-20).** The crate owns the editorial
   reductions the doc names (self-send zeroing, memo policy, rollups,
   ValueTransfer construction, display wording), extracted from
   zingolib's summary layer, PLUS the presentation of Mixnet Mode's
   tri-state for consumers. zingolib keeps canonical, key-aware
   classification and gains no dependency on the crate (opt-in).
2. **Compatibility mechanism = extension traits, names preserved
   (2026-07-20).** zingo-viewmodel exposes extension trait(s)
   implemented on LightClient carrying today's exact method names and
   signatures (value_transfers, messages_containing, do_total_*, ...);
   the editorial types move over together with their JsonValue impls,
   so zingo-mobile's diff is one Cargo.toml line plus a use line and
   the JSON stays bit-for-bit. zingolib keeps no wrappers (a wrapper
   would need a dependency cycle).
3. **Boundary = full purification, one branch (option B, 2026-07-20).**
   Moves to zingo-viewmodel: the ValueTransfer family (types, JsonValue
   impls, construction incl. self-send zeroing), messages_containing
   (empty-memo filtering + text search), the three do_total_* rollups
   and finsight types, and Mixnet Mode presentation. Stays canonical in
   zingolib: TransactionSummary/note/coin/outgoing summaries,
   transaction_summaries(), is_wallet_address (already pub), balances.
   AND the text-only memo policy leaves the canonical layer in the SAME
   branch: TransactionSummary's memo fields re-type from
   Option<String> (text-only) to a raw representation, with the
   text-only interpretation applied in zingo-viewmodel; every
   TransactionSummary consumer (zingo-cli list/display, wallet tests)
   is audited in this review.
