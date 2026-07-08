# Test-owned chain caches snapshot completed setup; txid-returning scenarios snapshot early

Libtonode tests regenerate their regtest chains from genesis on every run. We
replace that per-run generation with chain caches: saved copies of the
Validator's data directory, loaded into a fresh Validator at launch via the
`load_chain`/`cache_chain` primitives that `zcash_local_net` already provides.
Caching is the default regime for every libtonode test (tests that exercise
mining behavior carry an explicit opt-out), and each test owns exactly one
cache keyed by its own name. The snapshot is taken at scenario-setup
completion for every scenario that returns no transaction identifiers to its
caller (`custom_clients`, `unfunded_client`, `faucet`, `faucet_recipient`):
their setup sends are embedded in the cache, and wallets recover their view
of those transactions by syncing the cached chain. Only
`faucet_funded_recipient` — the one constructor whose funding txids escape to
the test — snapshots early, at its internal `faucet_recipient` stage, and
replays its funding sends live so the returned txids are minted fresh each
run. Caches live in a gitignored `chain_caches/` directory at the repository
root, which the test container bind-mounts, so caches built in one
containerized run serve the next.

A cache is built inline by the test itself when it finds no cache, or when the
driver sets the `ZINGO_REGENERATE_CHAIN_CACHE` environment variable (scoped to
specific tests by ordinary nextest selection). A rebuild discards the old
cache outright; nothing is moved aside. Because `cache_chain` stops the
Validator before copying, the building run relaunches from the snapshot it
just wrote and continues from there, so every run — including the one that
builds the cache — reaches its assertions through the load-from-cache path,
and a broken snapshot fails on the run that created it. Each cache carries an
inputs manifest recording its chain-determining inputs (activation heights,
miner pool, validator process and version, infrastructure dependency
revision); a mismatch at load time is treated as a miss and triggers a
rebuild, so consensus-parameter drift — imminent on this branch as the
ironwood migration moves activation heights — cannot silently serve a chain
mined under old rules.

## Considered Options

Keying caches by chain shape (the tuple of chain-determining setup inputs)
instead of by test name was designed and deliberately deferred: it
deduplicates the many byte-equivalent chains that tests sharing a scenario
signature generate, but it forces cross-process locking under nextest's
parallelism, whereas test-owned caches are never contended. Shape-keyed
deduplication remains the intended consolidation once the MVP has proven the
mechanism; the "chain shape" term is defined in `CONTEXT.md` for that purpose.

The decision as first recorded (earlier on 2026-07-07) snapshotted *every*
scenario at the send boundary — the point before setup's first
wallet-initiated transaction — to dodge the txid-return problem wholesale.
The same day's baseline instrumentation refuted the premise: setup costs 895
aggregate seconds across the 41 LocalNet tests, almost all of it in
post-send-boundary work (sends, confirmation mining, sync — the
`mine_to_transparent*` tests prove the send-free floor is ~4-6 s), and only
`faucet_funded_recipient` actually returns txids. A universal send boundary
would have recovered one to two minutes of the fifteen; snapshotting
completed setup for the txid-free scenarios recovers roughly twelve, at no
manifest cost. The amendment was adopted on that evidence. An outputs
manifest recording the funding txids remains the path to moving
`faucet_funded_recipient`'s snapshot past its sends, and its schema design
is still deliberately unblocked from the MVP.

Moving a superseded cache aside instead of discarding it was rejected because
chain generation is not byte-deterministic, so a kept copy serves only
speculative diagnostics; a driver who wants the old bytes can copy the
directory before regenerating.

## Consequences

- A cache-hit run's chain differs from a live-generated run's chain in
  non-consensus details (block timestamps, hashes), and setup transactions
  embedded in a cache keep their build-time txids across runs. No test can
  assert on those txids — the txid-free scenarios by definition never hand
  them out — but tests asserting on other non-consensus chain details must
  opt out; none are known to at the time of writing.
- The regenerate knob is deliberately not representable in source. An
  in-source per-test bool whose "on" state must never be committed would be a
  standing commit hazard; the environment variable plus nextest filtering
  expresses the same intent with nothing to revert.
- Per-test cache directories and the setup-metrics instrumentation both derive
  their names from the test thread's name, which `#[tokio::test]` sets to the
  test path. This holds for both runtime flavors (the root future polls on the
  test thread even under `multi_thread`), and the harness asserts the name is
  not a `tokio-runtime-worker` placeholder so a violation fails loudly rather
  than writing a garbage cache key.
- `cargo clean` does not touch `chain_caches/`; disk reclamation is manual
  deletion, which the miss-trigger makes always safe.
