# Live/offline twins: equivalence record

**Directive (2026-07-09, superseding 2026-07-08's verbatim
preservation):** each twin's body lives exactly once, as an
environment-generic fixture in `zingolib::testutils::twin_fixtures`,
instantiated offline against the mock indexer
(`zingolib::lightclient::mock_chain_tests`) and live against LocalNet
(`libtonode-tests/tests/unit_test_twins.rs`, behind the non-default
`unit_test_twins` feature, also reachable through the
`extra-credit-tests` bundle). The verbatim pre-unification originals
were deleted; git history is their accepted archive. This changes what
the live control detects: body drift between twin and original is now
impossible by construction, so a live/offline divergence implicates
the environment, never the test body — and, symmetrically, a weakening
edit to a fixture weakens both sides at once, a risk accepted as a
review-discipline concern rather than a reason to keep a second copy.
Run the live instantiations with
`cargo nextest run -p libtonode-tests --features unit_test_twins`.

**Directive (2026-07-08, historical):** the eight portable libtonode
tests gain offline twins; the live originals are never removed. After
the side-by-side runs below, the originals moved into their permanent
home in `unit_test_twins.rs`. The equivalence record below dates from
this regime: it compares the verbatim live originals against
hand-ported twin bodies, a comparison the 2026-07-09 unification made
moot for the five fixture-backed tests (the three tier-1 twins keep
distinct bodies and remain covered by verdicts 1–3 as written).

**Census note:** the move takes the eight originals out of the default
suite, so the default non-ignored census drops by eight relative to the
protection audit's 274 (they remain runnable behind the gate — since
2026-07-09 as fixture instantiations, not verbatim copies). The offline
twins run in zingolib's default unit suite. Exception: while the
`from_t_z_o` offline twin is `#[ignore]`d on zingolabs/zingolib#2447,
a default-suite live instantiation of its fixture stands in
`concrete.rs`, alongside the gated control.

**The twins' two hosts.** Tier 1 (three tests) runs on the synthetic
wallet rig alone: fabricated spendable funds, real proposal/build logic,
no network of any kind. Tiers 2–3 (five tests) run on the **stateful
mock indexer** (`zingolib/src/testutils/mock_indexer.rs`): an in-process
`CompactTxStreamer` server over a fabricated chain, so the wallet's REAL
pipeline — `GrpcIndexer`, pepper-sync scanning, record building, spend
bookkeeping, transparent self-receipts — runs end to end with no zebrad
or zainod. Funding transactions are built (not faked) by synthetic
faucet wallets through the build-without-broadcast seam, so their
outputs decrypt and spend like real ones.

## Systematic differences (apply to every twin)

1. **The mock validates nothing.** Proof validity, signature checks, fee
   floors, double-spend rejection, and boundary-adjacent verdicts are
   exercised only by the live suite (and, for the branch-id boundary,
   by the gap-4 unit fence in `send::built_transaction_shape` and the
   live `tip_spend_rejection` suite).
2. **Activation schedule.** The mock/synthetic chains activate
   everything at height 1; the live harness activates NU6.1/NU6.2 at
   height 5. Twins therefore never exercise upgrade boundaries — by
   design; boundary behavior has dedicated coverage.
3. **Coinbase is inexpressible.** Mock funding is ordinary
   transactions (compact indices deliberately start at 1 so nothing is
   mistaken for a coinbase); coinbase maturity and reward economics
   stay live-only.
4. **Fabrication artifacts.** Block heights are renumbered to the mock
   layout; txids differ (nondeterministic anyway); the fee a RECIPIENT
   sees on a funding wave reflects the mock faucet's fresh, unfragmented
   note pool.

## Per-test verdicts

| # | live original (libtonode concrete.rs) | offline twin (zingolib src) | verdict |
|---|---|---|---|
| 1 | basic_transactions::send_and_sync_with_multiple_notes_no_panic | proposal_shape::payment_no_single_note_covers_gathers_both_and_changes | EQUIVALENT-CORE, twin sharper at proposal level |
| 2 | slow::sapling_dust_fee_collection | proposal_shape::sapling_dust_is_not_collected_toward_fees | EQUIVALENT-CORE |
| 3 | fast::mine_to_transparent_and_shield | built_transaction_shape::four_coin_shield_builds_and_nets_input_minus_fee | NARROWED-BUT-SHARPENED; live stays load-bearing |
| 4 | slow::zero_value_receipts | mock_chain_tests::zero_value_receipts | EQUIVALENT (assertion-identical) |
| 5 | slow::list_value_transfers_check_fees | mock_chain_tests::list_value_transfers_check_fees | EQUIVALENT (assertion-identical) |
| 6 | slow::self_send_to_t_displays_as_one_transaction | mock_chain_tests::self_send_to_t_displays_as_one_transaction | EQUIVALENT (assertion-identical) |
| 7 | slow::send_to_transparent_and_sapling_maintain_balance | mock_chain_tests::send_to_transparent_and_sapling_maintain_balance | EQUIVALENT-CORE, one documented literal divergence |
| 8 | slow::from_t_z_o_tz_to_zo_tzo_to_orchard | mock_chain_tests::from_t_z_o_tz_to_zo_tzo_to_orchard | EQUIVALENT (assertion-identical, full 16-step ledger) |

**1 — multi-note gathering.** The twin asserts strictly more than the
live original at proposal time: exact selection (both 40_000 notes),
fee equal to the fee table, and the 20_000 change the live test pins
post-confirmation. Live-only residue: zebra accepting the two-input
bundle and the balance arriving through a real scan.

**2 — sapling dust.** The twin pins dust exclusion at selection plus
the exact fee and the 40_000 closing value, derived at proposal time.
The live original funds the dust note through a real cross-pool send;
the twin fabricates it directly. Same asserted property.

**3 — transparent shield.** The twin proves the four-coin shield
BUILDS (first offline shield build) and nets exactly sum − 30_000 into
orchard via the bundle's value balance. It cannot express the live
test's coinbase provenance (mining, maturity, reward totals), and it is
immune to the live test's documented intermittent shield-eligibility
race — which is exactly why the live original remains load-bearing: it
is the only place coinbase shielding and that race are observable.

**4 — zero-value receipts.** Assertion-for-assertion identical
(balances, the three value-transfer pins, including the single
Received{0, Orchard} entry), through the real scan pipeline. The live
original additionally proves zebra relays a zero-value output.

**5 — value-transfer fees.** Identical balance and composite-fee
(25_000) assertions; the twin's self-receipts (own taddr, own sapling)
arrive through genuine scanning of mock blocks, exercising the same
wallet paths.

**6 — self-send display.** Identical flow (incoming mixed send mined in
the same block as the wallet's own mixed self-send) and the same
txid-uniqueness contract.

**7 — maintain balance.** The full TransactionSummary-equality pinning
survives, including the Transmitted(target)→Confirmed transition of an
unmined send and the abandon-art recipient encodings, at renumbered
heights. ONE literal diverges: the second funding wave's recipient-side
fee is Some(10_000) offline versus Some(20_000) live — that number
belongs to the live faucet's fragmented note pool, not to recipient
behavior. Live-only residue: real mempool timing across the
mid-flight assertions.

**8 — pool-promotion ledger.** All sixteen steps carry over: every
funding source, both shields (including the two-coin shield), the two
InsufficientFunds refusals with identical shortfall numbers (20_000 and
60_000 against available 0), per-step balances, and the cumulative
205_000 confirmed-fee total. Live-only residue: zebra accepting each of
the twelve broadcasts.

**Status (2026-07-08): `#[ignore]`d pending zingolabs/zingolib#2447.**
This twin's step-1 funding is purely transparent, and pepper-sync's
SUBTRACTIVE `darkside_test` feature deletes transparent-address
discovery at compile time. Cargo feature unification enables that
feature for every crate co-built with darkside-tests, so the twin fails
deterministically in multi-package invocations (`makers test packages`,
`--workspace`) while passing in `-p zingolib` ones — root-caused via the
mock's taddr-request ledger (empty in failing builds, populated in
passing ones) and reproduced both directions on one host. The twin
itself is sound: it runs green solo via `--run-ignored`. Un-ignore when
#2447 converts the feature to runtime configuration. The same landmine
would strip transparent discovery from the libtonode live suite in any
whole-workspace invocation; the live originals are unaffected in the
packages/live partition because darkside never co-builds with them
there.

## Side-by-side runs (2026-07-08, host stack, this machine)

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
(`--features unit_test_twins`): 8/8 pass, 275s wall clock — the
relocation itself is verified, not assumed.

Both sides green on the same tree, same day. The aggregate cost ratio:
the eight twins total ~191s (dominated by proving and repeated sync
rounds, no processes spawned); the eight live originals total ~960s of
test time across the parallel 244s wall clock, each spawning a
zebrad + zainod pair. The twins' arithmetic matched the live pins
without adjustment on first passing run — including every
transaction-summary literal in test 7 except the documented
faucet-economics fee.
