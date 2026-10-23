# zingolib

Zcash light-wallet library, CLI, and integration-test suites.

## Language

**Core stack** / **Validator** / **Indexer**:
Defined in the infrastructure repo's `CONTEXT.md`
(github.com/zingolabs/infrastructure). This repo uses those terms with
identical meaning and does not redefine them.

**Faucet**:
The test client whose spend capability holds the funds a scenario starts
from: the mock chain's mined rewards, or the testnet funding seed.
_Avoid_: miner client, funded client

**Chain-bound test**:
A test whose behaviour under test involves chain interaction (mining,
mempool acceptance, indexer ingestion, or sync), run on the mock chain or
as a testnet cell. Contrast tests whose wallets come from the
synthetic-wallet builder and run offline.
_Avoid_: LocalNet test, online test, live test, network combo

**Mock chain**:
The in-process chain and indexer (`MockNet` over `MockChain`) that serves
the wallet's real sync and send pipeline without a validator process.
It enforces every consensus rule it models on submission, including
proofs, signatures and value balance (ruled 2026-10-15), so a guarantee
needs a live network only when it concerns network policy or a real
indexer's behaviour.
_Avoid_: mock indexer (that names the serving half alone), fake chain

**Testnet cell**:
A chain-bound test behind the non-default `testnet` feature. It is
compiled on every pull request and run against the testnet indexer the
suite names: the public `testnet.zec.rocks` node until the zingolabs
zainod testnet node exists (interim, ruled 2026-10-22), and that node
alone once it does: a tagged zaino release, serving TLS, with NU6.3
active (ruled 2026-10-15).
_Avoid_: testnet test, networked test, staged test

**Guarantee**:
A named claim that a group of a test's assertions pins together. Each
guarantee has exactly one home: the mock chain, a testnet cell, zaino's
suite, or retirement.
_Avoid_: assertion (that names one `assert!` call), check, invariant

**Twin**:
The mock-chain test that carries every guarantee of one chain-bound
test, sharpening but never narrowing any of them.
_Avoid_: offline twin, port, replacement test

**Binary e2e test**:
A test that exercises the shipped zingo-cli binary through its public CLI
surface against a public network's Indexer. Contrast tests that drive the
wallet in-process through `LightClient`.
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
