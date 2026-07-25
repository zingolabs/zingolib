# Test-owned chain caches snapshot completed setup and record returned outputs

Libtonode tests regenerate their regtest chains from genesis on every run. We
replace that per-run generation with chain caches: replay records of the
blocks a scenario's setup generated, read out of the running Validator over
JSON-RPC (`getblock` at verbosity 0, one hex block per line) and resubmitted
to a freshly launched Validator through `submitblock` on later runs. The
Validator revalidates every replayed block, and the Indexer ingests them
through the same flow live mining uses, so a cache-hit run's network differs
from a live run's only in where the blocks came from. Caching is the default
regime for every libtonode test (tests that exercise mining behavior carry an
explicit opt-out), and each test owns exactly one cache keyed by its own
name. Each cache snapshots its scenario at setup completion: setup sends are
embedded in the cache, and wallets recover their view of those transactions
by syncing the cached chain. `faucet_funded_recipient`, the one constructor
whose funding txids escape to the test, additionally records the identifiers
its build-time sends minted in the cache's `outputs.json`, written under the
same atomic rename as the blocks. A warm run replays the chain those
transactions are in and returns the recorded identifiers. The stage recorded
in the inputs manifest carries the funding amounts, so a test that changes
what it asks for discards its cache automatically. Caches live in a
gitignored `chain_caches/` directory at the repository root, which the test
container bind-mounts, so caches built in one containerized run serve the
next.

A test builds its cache inline when it finds none, or when the driver sets
the `ZINGO_REGENERATE_CHAIN_CACHE` environment variable (scoped to specific
tests by ordinary nextest selection). A rebuild discards the old cache
outright and moves nothing aside. A build run is a live run plus the export:
it generates its chain as always, writes the replay record at the snapshot
point, and continues on its own live net. The record is assembled in a
`.building` sibling and atomically renamed, so a crashed build never leaves
a half-cache. Every warm run exercises the replay path, and because replayed
blocks pass back through full `submitblock` validation, a corrupt cache
fails loudly at load rather than silently skewing assertions. Each cache
carries an inputs manifest recording its chain-determining inputs (setup
stage, activation heights, miner pool, validator and indexer identity). A
mismatch at load time counts as a miss and triggers a rebuild, so
consensus-parameter drift (imminent on this branch as the ironwood migration
moves activation heights) cannot silently serve a chain mined under old
rules.

## Considered Options

We designed, and deliberately deferred, keying caches by chain shape (the
tuple of chain-determining setup inputs) instead of by test name. Shape
keying deduplicates the many byte-equivalent chains that tests sharing a
scenario signature generate, but it forces cross-process locking under
nextest's parallelism, whereas test-owned caches are never contended.
Shape-keyed deduplication remains the intended consolidation once the MVP
has proven the mechanism, and `CONTEXT.md` defines the "chain shape" term
for that purpose.

The decision as first recorded (earlier on 2026-07-07) snapshotted *every*
scenario at the send boundary (the point before setup's first
wallet-initiated transaction) to dodge the txid-return problem wholesale.
The same day's baseline instrumentation refuted the premise: setup costs 895
aggregate seconds across the 41 LocalNet tests, almost all of it in
post-send-boundary work (sends, confirmation mining, sync). The
`mine_to_transparent*` tests prove the send-free floor is ~4-6 s, and only
`faucet_funded_recipient` actually returns txids. A universal send boundary
would have recovered one to two minutes of the fifteen, whereas snapshotting
completed setup for the txid-free scenarios recovers roughly twelve, at no
manifest cost. We adopted the amendment on that evidence, with
`faucet_funded_recipient` alone holding an early snapshot until the outputs
manifest existed. That manifest landed once the replay mechanism was proven:
`outputs.json` records the scenario's returned identifiers atomically with
the blocks, closing the last ~180 aggregate seconds of live setup sends the
early snapshot had preserved.

The mechanism as first implemented copied the Validator's data directory via
`zcash_local_net`'s `cache_chain`/`load_chain` primitives, with the building
run relaunching from its own snapshot. The first cold run (2026-07-08)
refuted that design for this stack: zebra holds every block within its
finalization depth of the tip in its in-memory non-finalized state (zebra's
`MAX_BLOCK_REORG_HEIGHT`, mirrored in this workspace as
`pepper_sync::sync::MAX_REORG_ALLOWANCE`, 100 as this record was written),
so a copied data dir of a 4-6-block setup chain contains essentially
genesis, and `Zebrad::stop()` is a SIGKILL, so even the finalized portion is
copied without a graceful flush. The relaunched Validator served a chain
zainod could not index ("could not determine best chain"). The
infrastructure repo's own zebrad cache generator corroborates the boundary:
it mines 150 blocks (past the depth as then understood) before calling
`cache_chain`. State-directory caches are therefore viable only for chains
longer than the finalization depth, and we adopted block replay in their
place. We dropped the builder-relaunch property with them: replay is
ordinary `submitblock` plus standard convergence rather than a state
transplant, so warm-run exercise suffices. A 2026-07-25 check of the zebra
6.0.0 tag found `MAX_BLOCK_REORG_HEIGHT` raised to 1000. The wider window
only strengthens this conclusion, and `MAX_REORG_ALLOWANCE` still reads
100, so the workspace mirror awaits reconciliation.

We rejected moving a superseded cache aside instead of discarding it: chain
generation is not byte-deterministic, so a kept copy serves only speculative
diagnostics, and a driver who wants the old bytes can copy the directory
before regenerating.

The first fully-observed warm run (2026-07-08, with the pipeline
observatory armed) adjudicated the replay design's last wrong assumption: a
freshly launched regtest Validator is at height 1 rather than 0, because
`Zebrad::launch` mines one block (the launch block) to prove the mining
service. Replay therefore works by competition rather than by appending:
the cached branch (always at least 3 blocks) forks around or duplicates the
launch block and wins the reorg. We fixed three points of the design on
that evidence. First, `submitblock`'s "duplicate" verdict counts as
acceptance, because transparent-pool regtest blocks are byte-deterministic,
so the cached and launch-mined block 1 can be the same block. Second, the
replay preflight expects a height of at most 1. Third, after submitting the
blocks the replay waits until the Indexer has converged to the replayed
tip, and only then may a wallet sync. The Indexer starts ingesting within
milliseconds of launch, so without that wait it would briefly serve the
orphaned launch block to any wallet that synced early, which is exactly how
the matrix_young pair failed.

## Consequences

- A cache-hit run's chain differs from a live-generated run's chain in
  non-consensus details (block timestamps, hashes), and setup transactions
  embedded in a cache keep their build-time txids across runs. No test can
  assert on those txids (the txid-free scenarios by definition never hand
  them out), but tests asserting on other non-consensus chain details must
  opt out. None are known to at the time of writing.
- The cache medium is human-auditable: `blocks.hex` is the chain itself, one
  serialized block per line, and any zcash tooling that parses raw blocks can
  inspect it. Nothing validator-internal (database format, state layout) is
  ever stored, so validator upgrades cannot corrupt caches. At worst a new
  validator rejects an old block on replay, which is a loud rebuild signal.
- The regenerate knob is deliberately not representable in source. An
  in-source per-test bool whose "on" state must never be committed would be a
  standing commit hazard. The environment variable plus nextest filtering
  expresses the same intent with nothing to revert.
- Per-test cache directories and the setup-metrics instrumentation both derive
  their names from the test thread's name, which `#[tokio::test]` sets to the
  test path. This holds for both runtime flavors (the root future polls on the
  test thread even under `multi_thread`), and the harness asserts the name is
  not a `tokio-runtime-worker` placeholder, so a violation fails loudly rather
  than writing a garbage cache key.
- `cargo clean` does not touch `chain_caches/`. Disk reclamation is manual
  deletion, which the miss-trigger makes always safe.
