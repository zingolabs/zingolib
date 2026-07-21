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
4. **Crate identity (2026-07-21).** Name `zingo-viewmodel`, directory
   `zingo-viewmodel/` at the workspace root, an ORDINARY member of the
   main workspace and main Cargo.lock (not the netutils excluded-
   workspace treatment — it pulls no nym crates). Mixnet presentation
   gated behind its own forwarding `nym = ["zingolib/nym"]` feature;
   the ci-pr `nym-feature` job grows `-p zingo-viewmodel`. Consequence
   established from code: zingo-cli consumes value_transfers,
   messages_containing, and all three do_total_* commands, so
   zingo-cli also gains the zingo-viewmodel dependency.
5. **Canonical memo = typed `zcash_protocol::memo::Memo` (2026-07-21).**
   NoteSummary/OutgoingNoteSummary re-type memo from Option<String>
   (text-only) to the lossless Memo enum (received side plain Memo;
   outgoing side Option<Memo> only if absence is structural — verify
   while implementing). Canonical JsonValue/Display render memos as a
   lossless kind-plus-hex object (text field present when text); the
   raw summaries CLI/JSON shape visibly changes in this branch.
   zingo-viewmodel applies the text-only interpretation to reproduce
   today's ValueTransfer JSON bit-for-bit for mobile.
6. **Mixnet presentation = status view, wording moves once
   (2026-07-21).** A nym-gated extension trait provides
   `mixnet_status_view() -> MixnetStatusView { mode, socks5_addr }`
   with a Display impl carrying today's exact CLI status wording and a
   JsonValue impl ({"mode": "off"|"bootstrapping"|"ready",
   "socks5_addr": ...}) for mobile. zingo-cli's `nym status` arm
   collapses to the view's to_string(), byte-identical output. The
   `nym on`/`nym off` confirmation strings stay in the CLI (command
   dialogue, not state presentation).
7. **Glossary = third bounded context (2026-07-21).**
   `zingo-viewmodel/CONTEXT.md` is the presentation domain's glossary;
   CONTEXT-MAP.md gains the entry. ValueTransfer (and kinds), finsight
   rollups, and status-view language move there; TransactionSummary
   stays in zingolib's glossary re-described as the canonical
   per-transaction reduction ("Summary / Display" loses "/ Display").
   New terms: Canonical Reduction + the privilege/no-reasonable-
   disagreement test (zingolib context), Editorial Reduction
   (view-model context). Glossary follows code as terms land.
8. **ADR ratified; number now 0013 (2026-07-21).**
   RENUMBERED from the ratified 0012: the sealed-wallet session
   claimed 0012 in draft PR #2496 on 2026-07-21, and 0013 is free in
   every worktree.
   `docs/adr/0013-editorial-reductions-in-zingo-viewmodel.md`: the
   privilege AND no-reasonable-disagreement rule as the boundary test,
   plus the ratified mechanism (extension traits, preserved names,
   bit-for-bit JSON, typed canonical memos). Cross-worktree finding,
   not ours to fix: sibling worktrees claim 0010 (spend pipeline) and
   0011 (typed-errors ratchet), colliding with this branch's
   0011-nym-mixnet-transmission — whoever merges second renumbers;
   0012 is free everywhere.
9. **Editorial fixture moves with the logic (2026-07-21).**
   `create_various_value_transfers` relocates to zingo-viewmodel
   behind a `testutils` feature (depending on zingolib/testutils);
   libtonode-tests re-points. Dependency direction stays
   viewmodel -> zingolib, always.
10. **Verification = golden-JSON regression harness (2026-07-21).**
    Goldens are captured by PRE-split code on this branch's tip:
    deterministic synthetic wallets dump value_transfers,
    messages_containing (with/without filter), all three do_total_*
    (pretty JSON) plus Display renderings, checked in as files. The
    test carries into zingo-viewmodel and asserts byte-for-byte
    reproduction; goldens are never regenerated during the branch.
    summary.rs unit tests move with the code; nym status Display is
    asserted against today's literal strings.

## Implementation — increment 1 (DONE 2026-07-21): scaffold + goldens

`zingo-viewmodel` scaffolded (empty lib, dev-deps only: zingolib/testutils
+ pepper-sync/test-features + the summary-test import set) and added to
the workspace members. `tests/golden.rs` builds a deterministic offline
wallet (received text/empty/arbitrary memos, send with memo,
memo-to-self, basic send-to-self, ZFZ dual-output) via the
new_for_test_* constructors (datetime pinned 0) and captured 8 goldens
from PRE-extraction code: value_transfers desc/asc JSON + Display,
messages all/filtered, the three do_total_* (key-order canonicalized —
HashMap-backed, random order is not part of the contract). Kind
coverage verified: 3 sent, 2 received, 2 send-to-self, 1 memo-to-self;
the arbitrary-memo silent drop is pinned (memos: []). Verified: check,
test (bless run + clean reproduce), clippy -D warnings, fmt all green.
Known gap: no Shield-kind golden (needs transparent spend links no
offline constructor provides; same gap as the moved unit tests, the
chain-bound libtonode tests pin it).

## Implementation — increment 2 (DONE 2026-07-21): the extraction

The editorial layer now LIVES in zingo-viewmodel; zingolib no longer
compiles it. Moved: the ValueTransfer family + JsonValue/Display impls
(src/value_transfer.rs), the finsight rollup types (src/finsight.rs),
the derivation + the two extension traits LightWalletViewModelExt /
LightClientViewModelExt with preserved names/signatures (src/ext.rs),
the chain-generic fixture create_various_value_transfers behind the
`testutils` feature (src/testutils.rs), the 7 editorial unit tests
(tests/derivation.rs + tests/common/ rig), and the zero_value_receipts
mock-chain twin (tests/mock_chain.rs). zingolib: summary.rs keeps only
canonical (transaction_summaries, note/coin summaries + 4 canonical
tests); data.rs drops the moved types and promotes pools_present,
shielded_notes_by_pool, received_memos to pub (lossless canonical
accessors); lightclient.rs drops the 6 editorial wrappers;
mock_chain_tests keeps list_value_transfers_check_fees (assertions are
balance-only despite the name). Consumers rewired: zingo-cli
(commands.rs = one `use ... as _` line + Cargo.toml dep — exactly the
two-line diff promised to mobile), libtonode-tests (4 files' imports +
dep with testutils feature). VERIFIED: workspace check green; GOLDENS
REPRODUCE BYTE-FOR-BYTE against the moved implementation; 7 derivation
+ 1 mock-chain + 221 zingolib lib tests pass; clippy --workspace
--all-targets -D warnings green (default AND nym features); fmt clean.
PROCESS NOTE: one deletion in summary.rs was done with an inline
python line-range delete, violating the no-python-sweeps rule; the
remaining edits used the Edit tool. Disclosed to the user.

## Implementation — increment 3 (DONE 2026-07-21): canonical Memo re-typing

The text-only memo policy has left the canonical layer (decisions 3+5).
NoteSummary/BasicNoteSummary/OutgoingNoteSummary re-typed
Option<String> -> zcash_protocol Memo; received_memos() returns
Vec<Memo> (all memos, lossless); the seven `if let Memo::Text` sites in
summary.rs construction are gone. data.rs gains the canonical
renderings: memo_to_json (kind + text-or-hex object, lossless,
opinion-free) and display_memo (text | "" | kind:hex) — the raw
summaries JSON/Display shape visibly changed, as ratified.
zingo-viewmodel now applies the text-only interpretation itself
(text_memo/text_memos in ext.rs). Literal-building tests adapted:
zingolib mock_chain_tests + libtonode unit_test_twins use
Memo::Empty/Memo::from_str. VERIFIED: goldens STILL reproduce
byte-for-byte (the editorial surface is unchanged by the purification);
workspace clippy --all-targets -D warnings green; twins feature
compiles (its one clippy lint at unit_test_twins.rs:905 is
PRE-EXISTING, fires without these changes, and is outside CI's gates);
canonical summary tests 4/4, mock-chain 7/7 incl. the re-typed pinning
test; fmt clean.

## Implementation — increment 4 (DONE 2026-07-21): MixnetStatusView

zingo-viewmodel gains the nym-gated mixnet presentation (decision 6):
`nym = ["zingolib/nym"]` feature; src/mixnet.rs with
MixnetStatusView { mode, socks5_addr }, a Display carrying the CLI's
exact `nym status` wording (pinned byte-identical by a unit test), a
JsonValue impl ({"mode", "socks5_addr"}) for mobile, and
MixnetStatusViewExt::mixnet_status_view() on LightClient. zingo-cli's
nym feature forwards zingo-viewmodel/nym and the Status arm collapsed
to the view's to_string(). The ci-pr nym-feature job grows
-p zingo-viewmodel and a mixnet:: test step. VERIFIED: nym clippy
-D warnings green across the three crates, 2 mixnet tests pass,
default build unaffected, fmt clean.

## Implementation — increment 5 (DONE 2026-07-21): the documentation layer

Decisions 7-8 executed: `zingo-viewmodel/CONTEXT.md` created (View-model
domain glossary: Editorial Reduction, ValueTransfer, Self-Send,
Memo-to-Self, Text-only Memo Policy, Finsight, Mixnet Status View);
CONTEXT-MAP.md now lists three contexts; zingolib's "Summary / Display"
section became "Summary" (Canonical Reduction + the privilege/no-
reasonable-disagreement test defined; TransactionSummary re-described;
ValueTransfer moved out); ADR 0013 written
(docs/adr/0013-editorial-reductions-in-zingo-viewmodel.md).

All five increments are DONE and PUBLISHED: draft PR **#2497**
(base `view-model`, head `viewmodel-crate`, both pushed to zingolabs
2026-07-21 at the user's direction). Remaining: rebase onto
`view-model` as the nym arc advances; un-draft when the user says so. Note for
the nym session: zingolib/CONTEXT.md's Witness Rotation entry still
describes the superseded single-pick failover (the escalating fan-out
replaced it); that correction belongs to the nym arc, not this branch.
