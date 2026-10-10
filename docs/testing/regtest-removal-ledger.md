# Regtest removal ledger

**Directive (2026-10-09):** every guarantee a regtest test provides moves
to a mock-chain test where the mock chain can carry it, and to a testnet
test where it cannot. The end state is a zingolib with no regtest at all.

## What the mock chain can and cannot do

`zingolib::testutils::mock_indexer::MockNet` controls the chain on cue:
it mines on demand (`mine_block`, `mine_mempool`, `mine_empty_blocks`),
mines a coinbase to a named miner (`mine_block_rewarding`), reorgs
(`reorg_to`), sets activation heights (`with_activation_heights`), holds
and promotes the mempool (`enter_mempool`, `promote_download_queue`),
serves mempool streams, subtree roots and `chain_metadata`, and injects
faults into named RPCs (`inject(Rpc, Fault)`: a failure status, a delay,
or a truncated stream). On submission its `validate` enforces the
consensus branch id, expiry, known anchors, unrevealed nullifiers,
transparent input ownership, coinbase maturity and the ZIP-317 fee, each
rule switchable.

It does not verify proofs, spend-authorization or binding signatures, or
the shielded value balance, and its serving behaviour is its own: an
assertion about a real indexer's lag, encoding or contract tests the mock
against itself. Those two residues are what only a live chain proves.

Each entry below lists the live test's assertions, what a mock twin needs
(or already has), and what cannot move to the mock.

## chain_generics.rs

**1. generate_a_range_of_value_transfers** and **2. send_shield_cycle.**
One fixture drives regtest, mock and testnet, so the mock twins
(`*_on_the_mock_chain`) are assertion-identical: value-transfer counts and
kinds, and `follow_proposal`'s per-step fee, output and confirmation-status
checks. Mock: nothing required. Not mockable: validator acceptance of the
built bytes (proofs, signatures, value balance).

## wallet.rs

**3. verify_old_wallet_uses_server_height_in_send.** Asserts the client's
fully scanned height equals the funded setup height plus five after a
send, so a send syncs to the server height first. Mock: fund, mine five
empty blocks, send, read `sync_state`; nothing new. Not mockable: nothing.

## observability.rs and sentinels.rs

**4. chain_mutates_only_via_owned_rpc**, **5. launch_mines_exactly_one_block**,
**6. transparent_launch_block_is_byte_deterministic**,
**7. suppressed_launch_generate_leaves_genesis.** These assert properties
of the `zcash_local_net` harness and of zebrad's launch: the RPC write log
is empty in an idle window, the tip fingerprint holds, the launch mines
exactly one block, that block is byte-deterministic. They test the
environment regtest removal deletes. Mock: meaningless. Testnet:
meaningless. Retire with regtest.

## mempool_attribution.rs

**8. indexer_mempool_view_trails_validator_acceptance.** Asserts zebra's
own mempool holds the txid right after `send_transaction`, the indexer's
`GetMempoolTx` shows it within a bound, and the lag is under the bound.
This measures a real indexer's latency behind a real validator. Mock:
lag is zero by construction, so nothing to assert. Testnet: the indexer
half is measurable (send, poll `GetMempoolTx`), the validator half has no
channel. Retire the validator half; the indexer half is a testnet probe
if the latency bound matters.

**9. wallet_mempool_record_trails_validator_acceptance.** Asserts the
sender's record reaches `Mempool` status within a bound. Mock twin:
`transmitted_transaction_has_mempool_status_before_mining` carries the
status; the time bound is real latency. Mock: done for the status. Not
mockable: the bound.

## migration.rs

**10. bound_note_reservation_and_external_spend_invalidation.** Asserts,
around a hand-built `MigrationState`: the reserved note stays unspent
while a free note covers an ordinary send, an external spend of the
reserved note marks part 0 `Invalidated`, a remainder at the sweep
minimum prompts no replan, and the migration completes with the disclosed
residual. Mock: two sends and `mine_mempool`; nothing new. Not mockable:
nothing.

**11. unavailable_boundary_tree_state_skips_without_sync.** With NU6.3
deferred and the chain inside the second bucket, asserts a transmit skips
(nothing sent, part `Assigned`, zero attempts, no witness) and never
syncs. The unavailability is the wallet's own: pepper-sync prunes the
boundary checkpoint once the tip is more than the retention past it.
Mock: done, as `boundary_pruning::unavailable_boundary_tree_state_skips_without_sync_on_the_mock_chain`,
with the same leap, real pruning, and a failure injected on the block-range
RPC that stays unconsumed, which proves the no-sync claim more directly
than the known-height check alone. Not mockable: nothing. The live test is
deleted in the same commit.

**12. two_phase_migration_end_to_end.** Asserts part count, `Complete`
phase, parts confirmed, migrated value, Ironwood balance, and the
value-transfer sum, with NU6.3 deferred past funding. Mock: activation
heights, pre-activation funding, mining while `migrate_to_ironwood`
awaits; the twin `migrate_to_ironwood_returns_under_continuous_sync`
holds the scaffold. Not mockable: validator acceptance of the parts.

**13. migrate_all_orchard_to_ironwood** and **14.
immediate_migration_chunks_a_fragmented_wallet.** Assert plan accounting,
transaction counts, migrated and residual balances, chunking into two
transactions, and the value-transfer sum. Mock twin
`immediate_migration_is_a_migration_value_transfer` covers the kind;
extend it with the balances and the chunk count. Not mockable:
acceptance.

## sync.rs

**15. add_subtree_roots.** Asserts the wallet's shard roots match the
server's subtree roots in count and bytes for Sapling and Orchard, and a
wallet stopped early holds fewer. Mock: serves subtree roots, so the
wallet-side bookkeeping ports. Not mockable: that the real indexer's
roots are right; that is a testnet comparison.

**16. sync_test.** A send to a transparent address, a shield proposal and
a sync with no assertion beyond success. Mock: trivial. Not mockable:
nothing.

**17. store_all_checkpoints_in_verification_window.** Asserts the three
shard trees retain every checkpoint across a dense ~112-block chain.
Mock twin `shardtree_roundtrip_restores_retained_checkpoints` holds the
scaffold; build the dense chain with `mine_block` carrying sends. Not
mockable: nothing.

**18. diagnose_subtree_root_stream.** A diagnostic of a real indexer's
stream truncation. Mock: `Fault::TruncateStream` reproduces the symptom,
not the server. Testnet: the real question. Retire or move to testnet.

**19. indexer_converges_with_validator_after_block_generation.** Asserts
the harness's convergence barrier. Retire with regtest.

**20. ironwood_notes_in_untracked_history_are_recovered** and **21.
orchard_…** Assert a wallet whose pool history was stripped reports a
failed sync on reopen, and recovers the note on rescan. Mock: wallet-file
manipulation plus `mine_block`; nothing new. Not mockable: nothing.

**22. served_outputs_match_chain_metadata_deltas.** Asserts the indexer's
`chain_metadata` tree-size deltas equal the outputs it serves, and that
Ironwood metadata first appears at the activation coinbase. Pure indexer
contract. Mock: tests the mock. Testnet: the right host, with testnet's
activation height. Move to testnet.

## tip_spend_rejection.rs (8 cells)

**23 to 30.** Every cell asserts `Verdict::Accepted` from zebra's own
judgement of the wallet's bytes: spends of tip-block notes, aged notes,
young and aged coinbase, to Orchard and Sapling, and near the activation
boundary. The suite pins the absence of a zebrad rc.0 mempool bug. Mock:
acceptance follows the mock's rules and proves nothing about zebra; the
wallet-side half (a tip-anchored spend is built at all) is a one-line
mock assertion. Not mockable: the verdict. Testnet: a tip-note spend is
reproducible (fund, wait one block, spend at once). Keep one testnet
cell as the sentinel; retire the rest.

## unit_test_twins.rs (8)

**31 to 38.** Ledgered in the equivalence record below; each has a mock twin
with its own recorded verdict. Mock: done. Not mockable: acceptance of
the bytes, which the 2026-07-21 live adjudication settled once.

## concrete.rs (22)

**39. unified_address_discovery.** Asserts new unified addresses are
absent before and discovered after sends to successive indices, across a
rescan. Mock twin `gap_address_compact_block_scanning` holds the
scaffold. Not mockable: nothing.

**40. ironwood_miner_coinbase_distribution**, **43. mine_to_ironwood**,
**44. mine_to_orchard**, **45. mine_to_transparent**, **61.
mine_to_transparent_coinbase_maturity.** Assert a miner wallet's per-pool
balances from coinbase rewards: block-one Sapling coinbase, Orchard
rewards before NU6.3 and Ironwood after, transparent rewards and their
maturity. Mock: `mine_block_rewarding` mints a coinbase to a miner and
`coinbase_reward_becomes_spendable_after_maturity` covers transparent
maturity; shielded coinbase outputs per activation era need the mock's
coinbase builder to mint to Orchard and Ironwood receivers, which is the
one capability to add. Not mockable: zebra's actual subsidy shape.

**41. received_tx_status_pending_to_confirmed_with_mempool_monitor.**
Asserts a received transaction is `Mempool(h)` before and `Confirmed(h)`
after mining. Mock: mempool streams and `mine_mempool`; the sender-side
twin exists, the receiver side is the same shape. Not mockable: nothing.

**42. utxos_are_not_prematurely_confirmed.** Asserts one UTXO unspent
before a shield, confirmed spent after, same output id. Mock: nothing
new.

**46. sync_all_expressible_epochs.** Syncs across every activation
boundary with no assertion beyond success. Mock: activation heights.

**47. test_scanning_in_watch_only_mode.** Asserts watch-only wallets
built from each viewing key see the sent values per pool, and a send
from a watch-only wallet fails with `CalculateTransactionError`. Mock:
nothing new.

**48. sends_to_self_handle_balance_properly**, **50. self_send**, **51.
check_list_value_transfers_across_rescan**, **49.
send_to_ua_saves_full_ua_in_wallet.** Assert value transfers, summaries
and outgoing-note recipient addresses are equal before and after a
rescan. Mock: `rescan` against the mock's blocks; nothing new.

**52. send_orchard_back_and_forth**, **53. send_mined_ironwood_to_ironwood**,
**58. send_pre_ironwood**, **59. send_post_ironwood.** Assert per-pool
balances after sends in each direction and era. Mock: activation heights
and, for the mined-Ironwood case, the shielded coinbase capability above.

**54. multi_input_sapling_send_with_orchard_change_no_panic** and **55.
mempool_spend_balance_and_note_status_accounting.** Assert total,
confirmed and unconfirmed balances, note spend statuses, and mempool
observation within thirty seconds. Mock: mempool streams; the time bound
is moot. Not mockable: nothing.

**60. propose_and_send_with_op_return_confirms_on_chain.** Asserts the
two-step proposal's fees, two txids, both confirmed, one input, two
outputs with no change, the OP_RETURN script and payload bytes. Mock:
transparent inputs and fees are validated; the bytes assertions read the
wallet's own transaction. Not mockable: zebra's standardness acceptance
of the null-data output. Keep on testnet.

**62. reload_wallet_after_short_sync.** Already a testnet test, ignored by
default.

## Totals

| disposition | tests |
| --- | --- |
| mock twin exists or is assertion-identical | 13 |
| portable to the mock with nothing new | 24 |
| portable once the mock mints shielded coinbase, now added | 7 |
| the assertion is about the real validator or indexer: testnet or retire | 18 |

The residue every mock test shares, validator acceptance of the wallet's
bytes, is one testnet test per transaction shape, or the mock verifying
bundles in `validate`.

## The 2026-07 twins: equivalence record

The record below is the file `live-offline-twins.md` as it stood when
the regtest removal directive subsumed it, under headings demoted one
level. It governs the eight tests of `unit_test_twins.rs`, entries 31
to 38 above.

## Live/offline twins: equivalence record

**Directive (2026-07-08):** the eight portable libtonode tests gain
offline twins; the live originals are never removed. After the
side-by-side runs below, the originals moved into their permanent home:
`libtonode-tests/tests/unit_test_twins.rs`, a non-default feature, a
module, and a file all named `unit_test_twins` (also reachable through
the `extra-credit-tests` bundle). Run them with
`cargo nextest run -p libtonode-tests --features unit_test_twins`.

**Amendment (2026-07-21, revised the same day):** the
`bump_and_check_pmc!` and `bump_and_check!` macros of
`list_value_transfers_check_fees` and
`from_t_z_o_tz_to_zo_tzo_to_orchard` (rows 5 and 8) bit-rotted during
the ironwood-era balance migration: each bound an `i:` argument but
expanded to `i: 0`, silently replacing every recorded ironwood
expectation with a zero assertion, invisible in default CI because the
file sits behind the off-by-default `unit_test_twins` feature. The
first remedy deleted both originals on the claim that the offline
twins carried the same ledgers; review of PR #2495 disproved that
claim. The ledgers diverge beyond the repaired column (the twin has
seventeen bump calls to the original's sixteen, and from step 4 onward
the original records `i: 45_000` where the twin asserts `i: 55_000`,
`s: 5_000 t: 5_000` against `s: 10_000 t: 10_000`, `i: 485_000`
against `i: 500_000`, through `s: 340_000` against `s: 395_000`; row
5's original records `i: 5_000` where the twin asserts `i: 15_000`).
Either the originals' recorded values were stale or the mock chain's
fee/ledger model diverges from a live chain, and only a live run could
adjudicate. Both originals were therefore restored with the
five-character macro repair (`i: 0` → `i: $i`), and three live
container runs adjudicated the dispute the same day. The chain ruled
for the twins' fee model: a V6 ironwood spend carries no separate
orchard-bundle-view charge. The originals' rewritten ledgers (which
had assumed that phantom charge and shrunk their send amounts to fit
it) were therefore corrected to the twins' amounts and arithmetic, and
both originals now pass live. One genuine live-mock divergence
surfaced at row 8's step 10: the live proposer selects ironwood alone
for the transparent-destination drain (sapling stays put) and refuses
the exact two-pool drain of 470_000 the mock accepts (exact drains
are pricing-shape-sensitive live), so the live original drains 465_000
at fee 15_000 and forks from the twin from that step onward, every
subsequent cell chain-adjudicated through the 190_000 fee total. The
prior "assertion-identical" verdicts for these rows were false and are
withdrawn; the fork is now the documented record.

**Second amendment (2026-07-21):** the first full sweep of the gated
suite since the ironwood-era default (ADR 0009) exposed that the *other*
balance-asserting originals had gone stale the same way: their twins
were updated when the era flipped, the gated originals were not. Rows
1, 3, 4, and 7's originals (`send_and_sync_with_multiple_notes`,
`mine_to_transparent_and_shield`, `zero_value_receipts`,
`send_to_transparent_and_sapling_maintain_balance`, plus
`sapling_dust_fee_collection`) were corrected to ironwood-pool
placement (every change a pool flip with fee arithmetic unchanged,
guided by their continuously-validated twins), and the whole suite now
passes live, 51 of 51. The lesson stands recorded: a gated suite's
assertions rot silently across era flips, and only its scheduled run
adjudicates them.

**Census note:** the move takes the eight originals out of the default
suite, so the default non-ignored census drops by eight relative to the
protection audit's 274 (they remain runnable, verbatim, behind the
gate). The offline twins run in zingolib's default unit suite.

**The twins' two hosts.** Tier 1 (three tests) runs on the synthetic
wallet rig alone: fabricated spendable funds, real proposal/build logic,
no network of any kind. Tiers 2–3 (five tests) run on the **stateful
mock indexer** (`zingolib/src/testutils/mock_indexer.rs`): an in-process
`CompactTxStreamer` server over a fabricated chain, so the wallet's REAL
pipeline (`GrpcIndexer`, pepper-sync scanning, record building, spend
bookkeeping, transparent self-receipts) runs end to end with no zebrad
or zainod. Funding transactions are built (not faked) by synthetic
faucet wallets through the build-without-broadcast seam, so their
outputs decrypt and spend like real ones.

### Systematic differences (apply to every twin)

1. **The mock validates nothing.** Proof validity, signature checks, fee
   floors, double-spend rejection, and boundary-adjacent verdicts are
   exercised only by the live suite (and, for the branch-id boundary,
   by the gap-4 unit fence in `send::built_transaction_shape` and the
   live `tip_spend_rejection` suite).
2. **Activation schedule.** The mock/synthetic chains activate
   everything at height 1; the live harness activates NU6.1/NU6.2 at
   height 5. Twins therefore never exercise upgrade boundaries, by
   design; boundary behavior has dedicated coverage.
3. **Coinbase is inexpressible.** Mock funding is ordinary
   transactions (compact indices deliberately start at 1 so nothing is
   mistaken for a coinbase); coinbase maturity and reward economics
   stay live-only.
4. **Fabrication artifacts.** Block heights are renumbered to the mock
   layout; txids differ (nondeterministic anyway); the fee a RECIPIENT
   sees on a funding wave reflects the mock faucet's fresh, unfragmented
   note pool.

### Per-test verdicts

| # | live original (libtonode concrete.rs) | offline twin (zingolib src) | verdict |
|---|---|---|---|
| 1 | basic_transactions::send_and_sync_with_multiple_notes_no_panic | proposal_shape::payment_no_single_note_covers_gathers_both_and_changes | EQUIVALENT-CORE, twin sharper at proposal level |
| 2 | slow::sapling_dust_fee_collection | proposal_shape::sapling_dust_is_not_collected_toward_fees | EQUIVALENT-CORE |
| 3 | fast::mine_to_transparent_and_shield | built_transaction_shape::four_coin_shield_builds_and_nets_input_minus_fee | NARROWED-BUT-SHARPENED; live stays load-bearing |
| 4 | slow::zero_value_receipts | mock_chain_tests::zero_value_receipts | EQUIVALENT (assertion-identical) |
| 5 | slow::list_value_transfers_check_fees | mock_chain_tests::list_value_transfers_check_fees | EQUIVALENT (assertion-identical; ledger adjudicated live 2026-07-21) |
| 6 | slow::self_send_to_t_displays_as_one_transaction | mock_chain_tests::self_send_to_t_displays_as_one_transaction | EQUIVALENT (assertion-identical) |
| 7 | slow::send_to_transparent_and_sapling_maintain_balance | mock_chain_tests::send_to_transparent_and_sapling_maintain_balance | EQUIVALENT (assertion-identical since 2026-07-21; the second-wave fee divergence closed with the era flip) |
| 8 | slow::from_t_z_o_tz_to_zo_tzo_to_orchard | mock_chain_tests::from_t_z_o_tz_to_zo_tzo_to_orchard | EQUIVALENT-CORE; step-10 ledger fork adjudicated live 2026-07-21 (live proposer drains single-pool, refuses the mock's exact drain) |

**1: multi-note gathering.** The twin asserts strictly more than the
live original at proposal time: exact selection (both 40_000 notes),
fee equal to the fee table, and the 20_000 change the live test pins
post-confirmation. Live-only residue: zebra accepting the two-input
bundle and the balance arriving through a real scan.

**2: sapling dust.** The twin pins dust exclusion at selection plus
the exact fee and the 40_000 closing value, derived at proposal time.
The live original funds the dust note through a real cross-pool send;
the twin fabricates it directly. Same asserted property.

**3: transparent shield.** The twin proves the four-coin shield
BUILDS (first offline shield build) and nets exactly sum − 30_000 into
orchard via the bundle's value balance. It cannot express the live
test's coinbase provenance (mining, maturity, reward totals), and it is
immune to the live test's documented intermittent shield-eligibility
race. That is exactly why the live original remains load-bearing: it
is the only place coinbase shielding and that race are observable.

**4: zero-value receipts.** Assertion-for-assertion identical
(balances, the three value-transfer pins, including the single
Received{0, Orchard} entry), through the real scan pipeline. The live
original additionally proves zebra relays a zero-value output.

**5: value-transfer fees.** Identical balance and composite-fee
(25_000) assertions; the twin's self-receipts (own taddr, own sapling)
arrive through genuine scanning of mock blocks, exercising the same
wallet paths. *Adjudicated live 2026-07-21 (see the amendment above):
the chain confirmed the twin's `i: 15_000`, and the repaired original
passes with the identical ledger.*

**6: self-send display.** Identical flow (incoming mixed send mined in
the same block as the wallet's own mixed self-send) and the same
txid-uniqueness contract.

**7: maintain balance.** The full TransactionSummary-equality pinning
survives, including the Transmitted(target)→Confirmed transition of an
unmined send and the abandon-art recipient encodings, at renumbered
heights. The one former literal divergence (the second funding wave's
recipient-side fee, Some(10_000) offline versus Some(20_000) live)
closed on 2026-07-21: the ironwood-era faucet normalization drains the
faucet into one consolidated note, so the fragmentation that made the
live wave a four-action transaction is gone and both records assert
Some(10_000). Live-only residue: real mempool timing across the
mid-flight assertions.

**8: pool-promotion ledger.** All sixteen steps carry over: every
funding source, both shields (including the two-coin shield), the two
InsufficientFunds refusals with identical shortfall numbers (20_000 and
60_000 against available 0), per-step balances, and the cumulative
205_000 confirmed-fee total. Live-only residue: zebra accepting each of
the twelve broadcasts. *Adjudicated live 2026-07-21 (see the amendment
above): steps 1-9 are assertion-identical under the chain-confirmed fee
model; from step 10 the ledgers fork deliberately, since the live
proposer drains ironwood alone (465_000, fee 15_000) where the mock
accepts the exact two-pool drain (470_000, fee 30_000). Every live cell
through the 190_000 fee total is chain-adjudicated. The fork is a
documented mock limitation: exact drains are pricing-shape-sensitive
live, and the mock's funding shapes evidently differ enough to mask
it.*

**Status (2026-07-08): `#[ignore]`d pending zingolabs/zingolib#2447.**
This twin's step-1 funding is purely transparent, and pepper-sync's
SUBTRACTIVE `darkside_test` feature deletes transparent-address
discovery at compile time. Cargo feature unification enables that
feature for every crate co-built with darkside-tests, so the twin fails
deterministically in multi-package invocations (`makers test packages`,
`--workspace`) while passing in `-p zingolib` ones. That was root-caused
via the mock's taddr-request ledger (empty in failing builds, populated
in passing ones) and reproduced both directions on one host. The twin
itself is sound: it runs green solo via `--run-ignored`. Un-ignore when
#2447 converts the feature to runtime configuration. The same landmine
would strip transparent discovery from the libtonode live suite in any
whole-workspace invocation; the live originals are unaffected in the
packages/live partition because darkside never co-builds with them
there.

### Side-by-side runs (2026-07-08, host stack, this machine)

Twins (one `cargo nextest run -p zingolib` invocation):

| twin | result | time |
|---|---|---|
| payment_no_single_note_covers_gathers_both_and_changes | PASS | 0.03s |
| sapling_dust_is_not_collected_toward_fees | PASS | 0.05s |
| four_coin_shield_builds_and_nets_input_minus_fee | PASS | 2.7s |
| zero_value_receipts (mock) | PASS | 20.3s |
| list_value_transfers_check_fees (mock) | PASS | 17.4s |
| self_send_to_t_displays_as_one_transaction (mock) | PASS | 27.7s |
| send_to_transparent_and_sapling_maintain_balance (mock) | PASS | 42.4s |
| from_t_z_o_tz_to_zo_tzo_to_orchard (mock) | PASS | 80.3s |

Live originals (one `cargo nextest run -p libtonode-tests` invocation,
zainod + zebrad per test; total wall clock 244s):

| live original | result | time |
|---|---|---|
| sapling_dust_fee_collection | PASS | 72.7s |
| mine_to_transparent_and_shield | PASS | 73.1s |
| list_value_transfers_check_fees | PASS | 87.1s |
| send_and_sync_with_multiple_notes_no_panic | PASS | 99.5s |
| self_send_to_t_displays_as_one_transaction | PASS | 99.7s |
| zero_value_receipts | PASS | 133.3s |
| send_to_transparent_and_sapling_maintain_balance | PASS | 149.8s |
| from_t_z_o_tz_to_zo_tzo_to_orchard | PASS | 244.3s |

After the move, the eight originals were re-run in their gated home
(`--features unit_test_twins`): 8/8 pass, 275s wall clock. The
relocation itself is verified, not assumed.

Both sides green on the same tree, same day. The aggregate cost ratio:
the eight twins total ~191s (dominated by proving and repeated sync
rounds, no processes spawned); the eight live originals total ~960s of
test time across the parallel 244s wall clock, each spawning a
zebrad + zainod pair. The twins' arithmetic matched the live pins
without adjustment on first passing run, including every
transaction-summary literal in test 7 except the documented
faucet-economics fee.
