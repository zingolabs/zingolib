# Ironwood (NU6.3) upgrade strategy: libtonode-tests first

Written 2026-07-09 from a survey of the release channels. Scope per the
directive: regtest `libtonode-tests` upgrades first, with test-result
pressure driving source changes on demand; everything else follows the
pattern this establishes.

## The version landscape (surveyed 2026-07-09)

| component | we run | latest ironwood-capable | delta |
|---|---|---|---|
| zebrad | 6.0.0-rc.0 | 6.0.0-rc.0 (2026-07-02, latest) | none — already current |
| zainod | 0.4.3-ironwood.1 (fork tag) | 0.6.0-rc.1 (2026-07-05) | two minors; RC changelog is dominated by ironwood work |
| zcash_protocol | 0.9.0 | 0.10.0-pre.0 (`NetworkUpgrade::Nu6_3` — "The Ironwood / NU6.3 network upgrade", mainnet height 4_134_000) | pre-release train |
| zcash_primitives | 0.28.0 | 0.29.0-pre.0 | pre-release train |
| zcash_client_backend | 0.23.0 | 0.23.0 — **no ironwood release exists yet** | blocked on upstream |

Two findings with strategic weight:

1. **The librustzcash ironwood train is incomplete on crates.io.**
   `zcash_protocol`/`zcash_primitives` have `-pre.0` cuts;
   `zcash_client_backend` does not, and its 0.23.0 pins protocol 0.9 —
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

## The phases — one variable at a time

**Phase 0 — reference freeze.** One recorded full run (default tier +
`extra-credit-tests`) at current pins. Archive `setup-metrics.jsonl`
and the run log as the pre-ironwood baseline. Cheap, already routine.

**Phase 1 — indexer bump, heights unchanged.** New docker-ci image:
zebrad stays 6.0.0-rc.0; zainod tracks zaino's `dev` lineage via the
per-commit image tags its CI publishes (`ZAINO_IMAGE_TAG` build-arg) —
`df443c9` today (the RC-cut content), advancing to the dev tip
(`17df071`, which carries a post-RC ironwood DB-migration fix) when its
image publishes. Per-commit sha tags, never rolling tags:
`ensure-image-exists` keys on tag presence, so a rolling tag would
neither rebuild nor reproduce. The `zaino-proto` crate rev advances to
the same sha. (makers-tasks lane owns the image.) Heights fixture untouched, so all
caches remain valid (zainod's version is deliberately not in the
manifest) and any failure is a zainod behavior change, not ironwood.
Gates: sentinels green, default tier 32/32. Bonus obligation: retest
the zaino#1386 findings against the RC — the parked convergence
regression test is the ready-made verifier; un-ignore it for one run.

**Phase 2 — librustzcash bump, heights still off.** Take the coherent
lrz set (pre-release pair now + `zcash_client_backend` when cut, or
one git pin). Expect mechanical breakage: `NetworkUpgrade` grows
`Nu6_3`, so exhaustive matches across zingolib/pepper-sync need arms;
the wallet/porter lanes own those files. The heights fixture still
keeps nu6_3 off, so behavior must not change: default tier green is
the gate, and any balance drift here is a bug, not ironwood. Check the
infrastructure repo's `zingo-consensus` for a matching bump (its
config writer already emits the NU6.3 key).

**Phase 3 — the flip (libtonode only).** `default_test_activation_heights`
gains nu6_3 at a fixture height chosen to match zaino's devtool
canonical set (above the launch block and the existing h2/h5 ladder).
On the first run every cache self-discards and rebuilds under ironwood
rules — by design, zero manual cache work. Then the pressure loop:
run the tier, triage each failure by lane —
- consensus branch id / tx construction above the boundary → wallet lane;
- subsidy or funding-stream arithmetic changes → scenarios balance
  constants (this lane), which is why they were centralized;
- compact-block serving or scanning gaps in the ironwood era → upstream
  zaino issues with observatory records attached (the #1386 pattern);
- launch/config contract changes → an infrastructure spec, the
  established agent pattern.
Iterate until 32/32; the sentinels adjudicate whether the launch block
and transparent determinism survive the new rules (they should — the
launch block precedes the activation height).

**Phase 4 — re-baseline.** Regenerate the metrics baseline, re-run the
extra-credit tier (twins + sentinels + slow gate) under ironwood, and
revisit the two deferred decisions this unblocks: committing the caches
(the heights churn that argued for waiting is over) and upstreaming the
sentinels alongside the observability trait.

## Sequencing rationale

Each phase moves one variable and has a green-suite gate, so a failure
names its cause by construction. The heights flip is deliberately last
and smallest: by then the binaries and libraries already speak
ironwood, and the flip itself is one fixture line plus an automatic
cache rebuild. Pressure flows outward from test results — nothing is
rewritten speculatively.
