# Test-suite protection audit: zingolabs/dev → upgrade_tests_atop_silent_fix

**Baseline (before):** `zingolabs/dev` at e4e731358.
**Subject (after):** d8cebac19, 78 commits ahead of the baseline (strict descendant).
**Census method:** `cargo nextest list --workspace` on each checkout; nextest
lists exactly the non-ignored, runnable tests. Both censuses used each
checkout's default features.

## Totals

| | before (dev) | after (HEAD) |
|---|---|---|
| Non-ignored tests, workspace-wide | **247** | **274** |

Composition of the change: 198 tests ran on both sides under the same name
and type; 49 census members left the suite (43 migrated to unit tests with
replacements, 3 renamed or retargeted in place, 3 deleted outright); 76
entered (57 zingolib unit tests, 3 pepper-sync regression tests, 10 new
live diagnostics, the 3 renames' new names, the revived mine_to_orchard,
the new orchard_miner_coinbase_distribution, and the indexer-convergence
barrier).

### Per binary

| binary | before | after |
|---|---|---|
| zingolib (unit) | 41 | 98 |
| zingo-cli (unit + log_file) | 77 | 77 |
| pepper-sync (unit) | 36 | 39 |
| libtonode concrete | 52 | 27 |
| libtonode chain_generics | 21 | 2 |
| libtonode sync | 1 | 2 |
| libtonode wallet | 1 | 1 |
| libtonode tip_spend_rejection | — | 8 |
| libtonode mempool_attribution | — | 2 |
| darkside | 6 | 6 |
| zingo-netutils / memo / status | 12 | 12 |

The e2e (LocalNet) share fell from 75 to 42 while the unit share rose from
166 to 216, plus 10 new live diagnostics and 6 unchanged darkside tests.

## Classification

### Unchanged type, ran before and after (198)

Verdict: **no unchanged test is weaker after the migration.** Every body
(and every shared fixture path the survivors run through) was compared
between the two revisions.

- **Stronger (3):** send_and_sync_with_multiple_notes_no_panic (gained the
  closing 20_000-change balance assert over the two-input spend),
  sends_to_self_handle_balance_properly (log-only with "TODO: Add asserts!"
  → post-shield balances, exact 15_000 shield fee, two-entry value-transfer
  shape, and byte-identical summaries across rescan), zero_value_receipts
  (log-only → 89_000 balance, three-entry value-transfer shape with the
  zero-value receipt pinned as one Received{0, Orchard}).
- **Recalibrated-equivalent (11):** the mining/balance family
  (mine_to_orchard, mine_to_transparent, mine_to_transparent_and_shield,
  mine_to_transparent_coinbase_maturity, send_orchard_back_and_forth,
  send_to_transparent_and_sapling_maintain_balance,
  received_tx_status_pending_to_confirmed_with_mempool_monitor,
  verify_old_wallet_uses_server_height_in_send) swapped zcashd-era magic
  balances for const-derived expressions (mined_block_rewards_total,
  FUNDED_FAUCET_SETUP_HEIGHT, POST_STREAM_BLOCK_REWARD) because the zebrad
  funding-stream economics changed the numbers; each still asserts exact
  equality on the same property. Both chain_generics survivors
  (generate_a_range_of_value_transfers, send_shield_cycle) run through the
  reworked with_assertions path: the mempool wait became a condition-poll
  with the same six-second ceiling, spends were separated from the chain
  tip, and a transient Transmitted status is tolerated within the existing
  patience bound instead of panicking immediately — the terminal
  assertions (recorded fees == confirmed fees, recorded outputs ==
  confirmed outputs, recipient Confirmed at send-height + 1) are intact,
  so nothing terminal was lost.
- **Same (the remainder of the 198):** byte-identical bodies, or purely
  mechanical changes — the ClientBuilder activation-heights constructor
  move (all 9 darkside tests), rustls-provider helper swaps, and
  sleep-to-poll conversions in zingo-cli's log_file tests whose decisive
  assertions are byte-identical. All 36 pre-existing pepper-sync unit
  tests and all pre-existing zingolib/zingo-cli unit tests are unchanged.

### Migrated libtonode → unit (43 census originals → 56 unit replacements)

Replacement rigs: `SyntheticWalletBuilder` (offline proposing over fabricated
notes with real trees/witnesses) and the record-fabrication rig (fabricated
`WalletTransaction`s for summary derivations).

Verdicts: **PRESERVED** (every asserted property carried), **PRESERVED+**
(replacement asserts strictly more), **NARROWED** (offline core carried;
a live half was dropped — named, with its surviving live cover where one
exists). No migration lost its asserted core.

Summary/record rig (→ wallet/summary.rs): filter_empty_messages
PRESERVED (both message counts); message_thread PRESERVED (thread counts
and both monotonic orderings); spendable_balance_includes_notes_in_incomplete_shards
PRESERVED (the incomplete-shard condition is now constructed explicitly
rather than produced incidentally); sapling_to_sapling_scan_together
PRESERVED (all summary fields; the name never had a scan-batching
assertion); by_address_finsight PRESERVED (memo-bytes accumulation "2"
then "6"); value_transfers → value_transfers_aggregation_and_ordering
PRESERVED (four-memo aggregation, idempotence and reverse-order
equalities, over a deliberately harder ordering input).
NARROWED in this family: create_send_to_self_with_zfz_active (both
kind-classification assertions carried, but the live pipeline's actual
injection of the ZFZ output is no longer exercised anywhere — see gaps);
sapling_incoming_sapling_outgoing (three record states carried; the
post-mine change-note bookkeeping dropped, covered by pepper-sync's
spend-status rig); send_funds_to_all_pools (the tri-pool balance assert
carried over fabricated notes; confirmed-balances-via-real-scan is
covered by the two surviving chain_generics fixtures).

Keys (→ wallet/keys.rs): address_generation_deterministic_and_coherent
and taddrs_from_old_seeds_stay_stable both PRESERVED byte-for-byte
(pinned encodings and seed vectors; the network was always scaffolding).

Proposal rig (→ lightclient/propose.rs, wallet/propose.rs):
ptfm_insufficient_funds PRESERVED (exact InsufficientFunds{10_000,
30_000}); ptfm_zero_value PRESERVED (ZeroValueSendAll);
toggle_zennies_for_zingo PRESERVED (max_send_value arithmetic);
propose_orchard_dust_to_sapling PRESERVED (Ok, FIXME carried);
four_coin_shield_proposal_shape PRESERVED (steps, 4 inputs, 30_000 fee,
sum-minus-fee change); t_incoming_t_outgoing_disallowed PRESERVED
(summary surfacing + refusal error chain).
PRESERVED+: dust_sends_change_correctly (original asserted only Ok; the
port adds fee-table and change arithmetic) and shield_transparent
(original was #[ignore]d and assertion-free — everything here is
net-new).
NARROWED: ptfm_general (drain contract sharpened to exact selection
[50_000, 100_000], dust line, zero change; live confirmed-drain covered
by chain_generics fixtures); send_not_fully_synced →
send_all_to_own_sapling_proposes (see the behind-tip caveat below);
proposal_targets_best_pool_per_unified_address (best-pool routing now
asserted directly on payment_pools; live scan-delivery dropped);
send_to_tex (two-step shape plus new ZIP-320 prior_step_inputs wiring;
the live broadcast is gone — see gaps); zero_value_change and
zero_value_change_to_orchard_created (zero-value change asserted at
proposal time with explicit arithmetic; post-mine bookkeeping covered by
pepper-sync); dust_inputs_are_ignored and
note_selection_covers_target_with_minimal_change (dust exclusion and
minimal-change selection sharpened; the fixtures' recorded-fee round
trips covered by the surviving fixtures).

Simpool (15): PRESERVED as a family — same fee tables, same funding
arithmetic, identical error-message strings ("Insufficient balance (have
…, need … including fee)"); three of fifteen spot-checked in full, the
helper is shared by all.

Pool matrix (2 degenerate proptests → 14 deterministic offline cases):
NARROWED-but-broadened. Each dev proptest ran ONE random case
(Config::with_cases(1), source pinned to sapling); the replacement
asserts the same fee/value/change arithmetic deterministically across
both shielded sources × three receiver pools × change/no-change, plus
the 546-zat transparent minimum and boundary rows. Dropped live half:
the per-case mempool transition and recipient scan round trip — that
class survives in send_shield_cycle and
generate_a_range_of_value_transfers, which drive the same
follow_proposal machinery live.

**Residual live-coverage gaps opened by the migration (consolidated):**
1. **Live sapling-source spend**: no surviving test spends a sapling
   note on a live chain (platform-forced; see deletions).
2. **Live ZFZ output injection**: zennies coverage is now entirely
   offline (classification + max_send_value); no live send with
   zfz=true remains (verified: zero zennies references in any live
   suite at d8cebac19).
3. **Live TEX broadcast**: the ZIP-320 two-step is asserted in proposal
   shape only; no live TEX round trip remains (verified likewise).
4. **Behind-tip broadcast**: preserved, but by exactly one test
   (verify_old_wallet_uses_server_height_in_send) — see the caveat
   under deletions.

### Renamed or retargeted within libtonode (3)

| before | after | commit |
|---|---|---|
| slow::note_selection_order | slow::multi_input_sapling_send_with_orchard_change_no_panic | 532b9ff2a |
| slow::send_mined_sapling_to_orchard | slow::send_mined_orchard_to_orchard | f68df2159 |
| fast::sync_all_epochs_from_heartwood | fast::sync_all_expressible_epochs | 90f73b3e9 |

- **send_mined_sapling_to_orchard → send_mined_orchard_to_orchard**: the
  observed invariant is preserved identically — a confirmation moves the
  mined-coinbase self-send from unverified to verified orchard balance —
  with only the coinbase source pool changed, because zebrad mines to
  orchard but not to the sapling pool. Justified retarget; the sapling-source
  aspect is inexpressible.
- **sync_all_epochs_from_heartwood → sync_all_expressible_epochs**: the
  staggered pre-Canopy activation boundaries were dropped because zebrad's
  config writer requires everything through Canopy active at height 1. The
  retained test sweeps the expressible NU5/NU6/NU6.1/NU6.2 boundaries.
  Platform-constrained, justified.
- **note_selection_order → multi_input_sapling_send_with_orchard_change_no_panic**:
  the original's nine assertions had been commented out for years (stale fee
  model, removed APIs); the live body's real contract — a two-input sapling
  spend with orchard change that zebra accepts — is now the test's name and
  doc comment. Selection ordering is asserted offline by
  note_selection_covers_target_with_minimal_change. No live protection lost;
  dead text removed.

### Deleted without replacement

Three census members were deleted with no replacement. Verdict on each,
from reading the dev bodies:

1. **fast::basic_scenario** (b2943a2b5). The whole body was one
   `faucet_funded_recipient_default(100_000)` call with every binding
   discarded and an in-source note that it was temporary scaffolding for
   infrastructure integration. Every scenario-based test performs the same
   setup before its own assertions, and the neighboring
   spendable-balance test asserts the same funding explicitly. Protection
   lost: none.
2. **slow::factor_do_shield_to_call_do_send** (26b0ee5e1). Sixteen lines
   that never shielded despite the name: one unwrapped 1_000-zat send to a
   transparent address. The z-to-t path is asserted live by
   self_send_to_t_displays_as_one_transaction and
   utxos_are_not_prematurely_confirmed, and arithmetically offline by the
   pool matrix's sends_to_transparent rows (exact fee, change, and payment,
   including the 546-zat dust minimum). Protection lost: none.
3. **slow::send_heartwood_sapling_funds** (fc4f77c9a). Mined coinbase
   directly into sapling and spent 3.5 ZEC of it into orchard on a live
   chain. The funding premise is inexpressible on the Core stack — zebrad mines
   to orchard (mine_to_orchard is live again) but not to the sapling
   pool — so the deletion is forced. The
   arithmetic is pinned offline by the sapling-source pool-matrix and
   simpool tests. **Residual gap, recorded honestly: no surviving test
   spends a sapling note on a live chain.** The last live sapling-source
   broadcast left the suite with this test; recovering it requires funding
   a sapling note by a send (not mining) and spending it, which no AFTER
   test currently does.

Two dev-side #[ignore]d tests (never census members) were also deleted by
fc4f77c9a: mine_to_sapling (mining to the sapling pool specifically is
unsupported — the dev ignore-reason's blanket "shielded pools" claim was
stale, as the revived mine_to_orchard shows — so the test can never run) and sync_all_epochs
(required pre-Canopy staggered activations zebrad cannot express, and hung
on shutdown; superseded by sync_all_expressible_epochs).

**Behind-tip broadcast caveat.** send_not_fully_synced's distinctive live
protection — zebra accepting a broadcast from a wallet behind the chain
tip — is NOT carried by its named migration target
(send_all_to_own_sapling_proposes, which is propose-only). It survives
solely in load_wallet::verify_old_wallet_uses_server_height_in_send, which
broadcasts from a wallet two blocks behind tip. That test is now
load-bearing for the whole class; the tip_spend_rejection and
mempool_attribution suites keep their wallets tip-synced and do not
substitute.

### Added

- 10 live diagnostics (tip_spend_rejection ×8, mempool_attribution ×2):
  new protection pinning zebra's boundary-adjacent orchard-output rejection
  and the indexer/wallet mempool-visibility lag.
- 3 pepper-sync spend_reset_lifecycle tests (b07a49673): regression fence for
  the silent-spend bug — a failed confirmed transaction must not destroy the
  spend observation.
- libtonode-tests::sync indexer_converges_with_validator_after_block_generation
  (c38089571): replaced the red-by-design lag diagnostic with a ten-round
  convergence-barrier assertion.
- fast::mine_to_orchard revived (was #[ignore]d on dev with a stale reason);
  fast::orchard_miner_coinbase_distribution new (exact coinbase distribution
  across pools at height 4).
- zingolib propose_100_000_to_self and send_all_to_own_sapling_proposes:
  net-new offline proposal coverage.
- proposal_shape::shield_transparent: net-new — the dev original was
  #[ignore]d (asserting nothing when it did run), so its census weight was
  zero; the offline port asserts step, input, and change shape.

### Ignore-status changes (census-visible)

- mine_to_orchard: ignored on dev → live on HEAD (revived, +1).
- diagnose_subtree_root_stream: added on HEAD already #[ignore]d (diagnostic;
  0 census weight on both sides).
- shield_transparent: ignored on dev (0 before) → replaced by a live unit
  port (+1 after, counted under migrations).

## Method and provenance

Change ledgers were extracted from `git log -p dev..HEAD` (56 test removals,
19 e2e additions, 60 unit additions, each attributed to a commit). The
protection verdicts come from reading both bodies of every changed test.
The dev census required CXXFLAGS="-include cstdint" (host GCC vs vendored
rocksdb 8.x).
