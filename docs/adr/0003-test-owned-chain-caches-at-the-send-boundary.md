# Test-owned chain caches snapshot mined-only setup at the send boundary

Libtonode tests regenerate their regtest chains from genesis on every run. We
replace that per-run generation with chain caches: saved copies of the
Validator's data directory, loaded into a fresh Validator at launch via the
`load_chain`/`cache_chain` primitives that `zcash_local_net` already provides.
Caching is the default regime for every libtonode test (tests that exercise
mining behavior carry an explicit opt-out), each test owns exactly one cache
keyed by its own name, and the snapshot is taken at the send boundary — the
point before setup performs its first wallet-initiated transaction. Everything
before the boundary is pure mining and comes from the cache; sends and their
confirmation mining replay live on every run. Caches live in a gitignored
`chain_caches/` directory at the repository root, which the test container
bind-mounts, so caches built in one containerized run serve the next.

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

Snapshotting at scenario-setup completion rather than at the send boundary was
rejected for the MVP because setup transactions mint txids at build time that
tests later assert against; returning them on a cache hit requires a
per-scenario outputs manifest whose schema design we chose not to block on.
The cost is real and accepted: for scenarios whose setup is mostly sends
(`faucet_funded_recipient` and kin), most of the per-run cost remains until an
outputs manifest moves the snapshot point past the sends.

Moving a superseded cache aside instead of discarding it was rejected because
chain generation is not byte-deterministic, so a kept copy serves only
speculative diagnostics; a driver who wants the old bytes can copy the
directory before regenerating.

## Consequences

- A cache-hit run's chain differs from a live-generated run's chain in
  non-consensus details (block timestamps, hashes). Tests asserting on such
  details must opt out; none are known to at the time of writing, since the
  MVP cache covers only launch plus the initial two-block generation.
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
