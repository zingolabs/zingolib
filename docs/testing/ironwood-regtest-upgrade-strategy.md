# Ironwood (NU6.3) upgrade strategy: libtonode-tests first

Written 2026-07-09 from a survey of the release channels. Scope per the
directive: regtest `libtonode-tests` upgrades first, with test-result
pressure driving source changes on demand; everything else follows the
pattern this establishes.

## The version landscape (surveyed 2026-07-09)

| component | we run | latest ironwood-capable | delta |
|---|---|---|---|
| zebrad | 6.0.0 (bumped 2026-07-16) | 6.0.0 (2026-07-10, final) | none, current again |
| zainod | 0.6.0-rc.1-no-tls (image tag, updated 2026-07-16) | 0.6.0 (2026-07-13; the zaino workspace's crates.io debut) | one patch level, deliberately held: 0.6.0 masks sendrawtransaction rejections (zaino#1404, open) |
| zcash_protocol | 0.9.0 | 0.10.0-pre.0 (`NetworkUpgrade::Nu6_3`, "The Ironwood / NU6.3 network upgrade", mainnet height 4_134_000) | pre-release train |
| zcash_primitives | 0.28.0 | 0.29.0-pre.0 | pre-release train |
| zcash_client_backend | 0.23.0 | 0.23.0, **no ironwood release exists yet** | blocked on upstream |

Update (2026-07-16): zebra published the 6.0.0 final on 2026-07-10,
one day after this survey, so the original "already current" verdict
went stale immediately. The pin (`.env.testing-artifacts`) now names
6.0.0. The rc.0 → final delta includes mempool-admission changes
(the mempool stays active through sync-status fluctuations, and script
verification moved to a shared thread pool), which is the subsystem
the `tip_spend_rejection` suite probes, so that suite's verdicts must
be re-observed under the new pin before its assertions are re-litigated.

Two findings with strategic weight:

1. **The librustzcash ironwood train is incomplete on crates.io.**
   `zcash_protocol`/`zcash_primitives` have `-pre.0` cuts;
   `zcash_client_backend` does not, and its 0.23.0 pins protocol 0.9,
   so a piecemeal crates.io bump cannot compile. The realistic phase-2
   options are (a) wait for the `zcash_client_backend` pre-release, or
   (b) git-pin the librustzcash workspace at the single revision the
   `-pre.0`s were cut from, taking the whole set coherently. (b) is the
   pattern this repo already uses for the infrastructure pin.
2. **zaino 0.6.0-rc.1 ships ironwood support with self-declared
   gaps.** Its own changelog gates its wallet-funding e2e family on
   "the ironwood scanning gap", notes zebra's missing V6 transaction
   `Arbitrary` generation, and aligns regtest heights to a "devtool
   canonical set". Expect the pressure loop to surface zaino gaps as
   upstream issues rather than local fixes; adopt their canonical
   regtest height set for cross-repo comparability when we flip.

## Why this suite is unusually ready

The machinery built this week was designed for exactly this event:
chain-cache manifests encode activation heights, so the heights flip
self-discards and rebuilds every cache with no manual step; the
sentinels (`--features sentinels`) pin the launch contract and
transparent block determinism, so environment bumps get a purpose-built
gate; the observatory attaches full traffic/state records to every
failure, so each red test arrives self-diagnosed; and the tiering keeps
the pressure loop's blast radius to the 32-test default tier.

## The phases: one variable at a time

**Phase 0: reference freeze.** One recorded full run (default tier +
`extra-credit-tests`) at current pins. Archive `setup-metrics.jsonl`
and the run log as the pre-ironwood baseline. Cheap, already routine.

**Phase 1: indexer bump, heights unchanged.** New docker-ci image:
zebrad stays at its pin (6.0.0 since the 2026-07-16 bump); zainod
tracks zaino's `dev` lineage via the
per-commit image tags its CI publishes (`ZAINO_IMAGE_TAG` build-arg),
which is `df443c9` today (the RC-cut content), advancing to the dev tip
(`17df071`, which carries a post-RC ironwood DB-migration fix) when its
image publishes. Per-commit sha tags, never rolling tags:
`ensure-image-exists` keys on tag presence, so a rolling tag would
neither rebuild nor reproduce. The `zaino-proto` crate rev advances to
the same sha. (makers-tasks lane owns the image.) The publish side
replicates zaino's dev method (zingolabs/zaino
`.github/workflows/build-n-push-ci-image.yaml`, content-addressed
tagging per zingolabs/zaino@d8754b72a): `build-n-push-ci-image.yaml`
rebuilds and pushes `zingodevops/ci-build:<content-tag>` whenever the
tag's hash inputs change, and the test/coverage workflows resolve the
same tag through the reusable `compute-image-tag.yaml`, so CI always
runs the image the working tree's pins describe. The hardcoded
`ci-build:011` reference is gone. Heights fixture untouched, so all
caches remain valid (zainod's version is deliberately not in the
manifest) and any failure is a zainod behavior change, not ironwood.
Gates: sentinels green, default tier 32/32. Bonus obligation: retest
the zaino#1386 findings against the RC. The parked convergence
regression test is the ready-made verifier; un-ignore it for one run.

**Phase 2: adopt PR #2428's dependency universe, heights still off.**
The open ironwood PoC (zingolabs/zingolib#2428, `feat/ironwood-migration`
→ `feat/ironwood`) already defines the coherent target set, and phase
two adopts it verbatim so the branches converge:

- **librustzcash**: `[patch.crates-io]` source-swap of the whole family
  (client_backend, address, encoding, history, keys, primitives,
  proofs, protocol, transparent, equihash, f4jumble) to
  `zcash/librustzcash` rev `4d9a68dc80508e7644aa99e1b4add7c831057bba`
  (canonical upstream main, pinned). That is the git-pin option,
  already chosen upstream of us.
- **zebra**: `zebra-chain`/`zebra-rpc`/`zebra-node-services` patched to
  the `zcashfoundation/zebra` branch `nu63-ironwood` (the ZIN-37
  ironwood-value-pool fork, built against orchard 0.15.0-pre.1). Side
  effect worth noting: zebra crates enter the dependency graph for the
  first time, which may let the max-reorg/finalization-depth constant
  become an import instead of a documented mirror.
- **Surroundings**: orchard 0.15.0-pre.1, zcash_address 0.13.0-pre.0,
  `zcash_primitives` gains the `non-standard-fees` feature, and
  `lightwallet-protocol` is patched to the fork rev carrying the
  Ironwood proto fields with `rebuild-proto` (build environments need
  `protoc`, a container-image requirement for the makers-tasks lane).
- **Compiler cfg**: ironwood sits behind `--cfg zcash_unstable="nu6.3"`
  RUSTFLAGS via the in-repo `.cargo/config.toml`, which the container
  inherits through the bind mount.

The proto fields settle a structural fact the phases below must plan
for: **Ironwood is a new shielded pool**, not just consensus-rule
changes. The proto adds `CompactTx.ironwoodActions`,
`TreeState.ironwoodTree`, `ShieldedProtocol.ironwood`, and
`ChainMetadata.ironwoodCommitmentTreeSize`, and PR #2428's pepper-sync
diff carries ironwood commitment trees, nullifiers, and OVKs alongside
sapling and orchard. Phase three's blast radius therefore includes the
pool vocabulary end to end: miner-pool options, the balance-assertion
macros' pool legs, scenario constants, and zaino's era-serving behavior.

Expect mechanical breakage in this phase: `NetworkUpgrade` grows
`Nu6_3` and the pool enums grow a variant, so exhaustive matches across
zingolib/pepper-sync need arms; the wallet/porter lanes own those
files. The heights fixture still keeps nu6_3 off, so behavior must not
change: default tier green is the gate, and any balance drift here is a
bug, not ironwood. Check the infrastructure repo's `zingo-consensus`
for a matching bump (its config writer already emits the NU6.3 key).

**Phase 3: the flip (libtonode only).** `default_test_activation_heights`
gains nu6_3 at a fixture height chosen to match zaino's devtool
canonical set (above the launch block and the existing h2/h5 ladder).
On the first run every cache self-discards and rebuilds under ironwood
rules, by design, so there is zero manual cache work. Then comes the
pressure loop. Run the tier and triage each failure by lane:
- consensus branch id / tx construction above the boundary → wallet lane;
- subsidy or funding-stream arithmetic changes → scenarios balance
  constants (this lane), which is why they were centralized;
- compact-block serving or scanning gaps in the ironwood era → upstream
  zaino issues with observatory records attached (the #1386 pattern);
- launch/config contract changes → an infrastructure spec, the
  established agent pattern.
Iterate until 32/32; the sentinels adjudicate whether the launch block
and transparent determinism survive the new rules (they should, since
the launch block precedes the activation height).

**Phase 4: re-baseline.** Regenerate the metrics baseline, re-run the
extra-credit tier (twins + sentinels + slow gate) under ironwood, and
revisit the two deferred decisions this unblocks: committing the caches
(the heights churn that argued for waiting is over) and upstreaming the
sentinels alongside the observability trait.

## Sequencing rationale

Each phase moves one variable and has a green-suite gate, so a failure
names its cause by construction. The heights flip is deliberately last
and smallest: by then the binaries and libraries already speak
ironwood, and the flip itself is one fixture line plus an automatic
cache rebuild. Pressure flows outward from test results, and nothing is
rewritten speculatively.
