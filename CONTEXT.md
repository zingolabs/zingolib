# zingolib

Zcash light-wallet library, CLI, and integration-test suites.

## Language

**Network combo**:
The Indexer+Validator pair a test scenario runs against, selected at compile
time in `zingolib_testutils`. The no-feature default is the Core stack
(zainod + zebrad); the only surviving alternative is lightwalletd + zebrad,
which dies with the Legacy stack.
_Avoid_: test stack, server pair

**Core stack** / **Legacy stack** / **Validator** / **Indexer**:
Defined in the infrastructure repo's `CONTEXT.md`
(github.com/zingolabs/infrastructure). This repo uses those terms with
identical meaning and does not redefine them. zcashd-backed network combos
were removed from this repo in July 2026; lightwalletd remains only for
darkside tests and the opt-in `test_lwd_zebrad` combo.

**Faucet**:
The test client whose spend capability receives the regtest Validator's
mining rewards, providing the funds most scenarios start from.
_Avoid_: miner client, funded client

**Chain shape**:
The complete set of inputs that determines the chain a test scenario's
setup generates: the scenario constructor and its chain-affecting
arguments (miner pool, activation heights, funding amounts) plus the
Validator that mines it. Tests with the same chain shape are meant to
observe equivalent chains. Chain shape is the natural key for sharing
chain caches between tests; today each test owns its cache outright,
and shape-based sharing is a foreseen consolidation.
_Avoid_: test's chain

**Chain cache**:
A saved copy of a regtest Validator's data directory, taken at a
scenario setup's send boundary and loaded into a fresh Validator at
launch to replace live generation of the mined-only setup. Each test
owns at most one cache, keyed by the test's name.
_Avoid_: chain state snapshot, cached chain state

**Send boundary**:
The point in a scenario's setup before its first wallet-initiated
transaction. Setup work before the boundary is pure mining and is
covered by the chain cache; setup work at or after the boundary replays
live on every run.

**Chain-bound test**:
A test that must run against a live network combo because the behavior
under test involves chain interaction — mining, mempool acceptance,
indexer ingestion, or sync. Contrast tests whose wallets come from the
synthetic-wallet builder and run offline. Chain caches and the
setup-metrics instrumentation apply only to chain-bound tests; the
metrics file is the census of them.
_Avoid_: LocalNet test, online test, live test (that names the Makefile
package partition, not this category)

**Phantom unspent note**:
A note the wallet offers as spendable although its nullifier is already
on-chain. Every proposal that selects one is rejected by the Validator as a
double-spend. Distinct from a pending-spent note, whose spend the wallet
knows about and correctly excludes from selection.
_Avoid_: stale note, stuck note
