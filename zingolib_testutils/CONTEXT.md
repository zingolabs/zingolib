# zingolib

Zcash light-wallet library, CLI, and integration-test suites.

## Language

**Network combo**:
The Indexer+Validator pair a test scenario runs against, fixed at the Core
stack (zainod + zebrad) in `zingolib_testutils`.
_Avoid_: test stack, server pair

**Core stack** / **Validator** / **Indexer**:
Defined in the infrastructure repo's `CONTEXT.md`
(github.com/zingolabs/infrastructure). This repo uses those terms with
identical meaning and does not redefine them. The legacy validator-backed
network combos were removed from this repo in July 2026, and the opt-in
Legacy-stack indexer combo followed with the darkside-tests retirement.

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
A replay record of the blocks a test scenario's setup generated, read
out of the regtest Validator at scenario-setup completion and
resubmitted to a fresh Validator at launch to replace live setup
generation. Scenarios that return transaction identifiers record them
in the cache alongside the blocks, so warm runs return identifiers that
name transactions in the replayed chain. Each test owns at most one
cache, keyed by the test's name.
_Avoid_: chain state snapshot, state-directory copy, send boundary
(a superseded interim design in which txid-returning scenarios
snapshotted before their sends; see ADR 0003's history)

**Launch block**:
The single block `zcash_local_net` mines inside `Zebrad::launch` on
regtest to prove the mining service before returning. Every freshly
launched Validator is therefore at height 1, not 0, and block 1's
coinbase (the pre-NU5 sapling payout the balance constants encode)
is minted at launch, not by test code. Chain-cache replay submits the
cached chain as a competitor to the launch block and wins the reorg.
_Avoid_: genesis block (that is height 0, the network's own)

**Chain-bound test**:
A test that must run against a live network combo because the behavior
under test involves chain interaction: mining, mempool acceptance,
indexer ingestion, or sync. Contrast tests whose wallets come from the
synthetic-wallet builder and run offline. Chain caches and the
setup-metrics instrumentation apply only to chain-bound tests; the
metrics file is the census of them.
_Avoid_: LocalNet test, online test, live test (that names the Makefile
package partition, not this category)

**Primed**:
The state of an observability instrument (a state watch or a front
record) whose recording window is open: the priming instant is time
zero for every event it records. Instruments are primed before the
observed process launches so the window covers the launch itself.
Priming is distinct from connection. A front record receives traffic
only once it is also connected to the process it observes; a
primed-but-unconnected record is valid and silently empty (how the
Legacy stack runs, whose Indexer accepts no observer).
_Avoid_: armed, registered (the superseded pair); started, created
(neither implies the open recording window)

**Binary e2e test**:
A test that exercises the shipped zingo-cli binary end-to-end through its
public CLI surface, driving it as a Wallet (in the infrastructure repo's
sense) against a live network combo. Contrast libtonode tests, which
drive the wallet in-process through `LightClient`.
_Avoid_: CLI live test, harness test

**Phantom unspent note**:
A note the wallet offers as spendable although its nullifier is already
on-chain. Every proposal that selects one is rejected by the Validator as a
double-spend. Distinct from a pending-spent note, whose spend the wallet
knows about and correctly excludes from selection.
_Avoid_: stale note, stuck note

**Boot Window**:
The interval of a benchmarked session from the moment the session process
is spawned to the moment the Sync Span opens. It is where a mixnet session
spawns its proxy, proves its quartet, and runs the Server-Selection Sweep,
so it holds the mixnet's start-up cost. Measured in wall-clock seconds and
in core-seconds per role.
_Avoid_: boot phase, startup span

**Scan Window**:
The interval of a benchmarked session that coincides with the Sync Span:
from the marker opening that span to the marker closing it. It holds the
scan and nothing else, so a cost charged to it is a cost the scan paid.
Measured in core-seconds per role, while the scan's own duration comes
from the engine's clock rather than from this window.
_Avoid_: sync window, span window
