# Claim: zebrad 6.0.0 bump (grilling session, 2026-07-16)

Session working directly in the `feat/ironwood-migration-zebrad-testing`
worktree, investigating the six failing `tip_spend_rejection` tests.
The user ratified bumping the zebrad test binary from 6.0.0-rc.0 to the
6.0.0 final release (published 2026-07-10, after the strategy doc's
2026-07-09 survey) and correcting the doc's version table.

The bump is deliberately its own single variable, per the strategy
doc's phase discipline: the 6.0.0 changelog names mempool-admission
changes, which is the subsystem the tip_spend_rejection suite probes.
The zainod pin (0.6.0-rc.1-no-tls) is untouched.

## Ratified follow-on (2026-07-16): attribute_send_failure

The user ratified extracting the dual-channel/dual-time attribution
probe prototyped by `boundary_rejection_attribution` into a reusable
`zingolib_testutils::attribution` helper, and rewiring the
`tip_spend_rejection` suite's error paths through it so every failing
cell self-diagnoses which layer (wallet builder, indexer transport,
validator verdict) produced its error. Rejection classification moves
from the indexer-path error string to the validator's direct verdict,
which makes the suite robust to the zainod 0.6.0 verdict-masking
regression (zaino#1404).

## Discovery run verdict (2026-07-16, zebrad 6.0.0 container run)

The boundary rejection is GONE under zebrad 6.0.0: the
boundary-adjacent orchard-output send was ACCEPTED in both the
attribution test's environment and the matrix cell that reproduced
the original pool_matrix failure. The mechanism was inside zebra's
rc.0 mempool admission, which 6.0.0 reworked. Separately, the four
run_cell tests were failing before the send — the
`faucet_funded_recipient` scenario now funds via PoolType::IRONWOOD,
so the funding note is an ironwood note and the cells' orchard-notes
predicate never matched; fixed by matching either pool.

A second run (2026-07-16) confirmed both observations deterministically
(38/40 green; only the two acceptance-observing phenomenon tests red),
so the suite was re-pinned: every cell now asserts Accepted,
`boundary_rejection_attribution` became the boundary-acceptance
sentinel driving `classify_send_outcome`, and the module doc carries
the round-five verdict. The stale cross-reference in
`mempool_attribution.rs` got a dated correction. The zainod audit
(same day) aligned the docker-ci fallback ARG to the canonical
0.6.0-rc.1-no-tls pin and corrected the strategy doc's zainod row;
crates.io's zainod 0.6.0 (2026-07-13) stays unadopted while
zaino#1404 is open.

The third container run (2026-07-16, user-driven) came back fully
green with the re-pinned suite, confirming the batch end to end. The
work is publication-ready: branch `feat/zebrad-6-boundary-fix`, three
staged commits, PR against feat/ironwood (messages and body drafted
in-session).

## File claims

- `.env.testing-artifacts` (ZEBRA_VERSION pin)
- `docker-ci/Dockerfile` (ZEBRA_VERSION ARG fallback default)
- `docs/testing/ironwood-regtest-upgrade-strategy.md` (version table
  correction)
- `zingolib_testutils/src/attribution.rs` (new module)
- `zingolib_testutils/src/lib.rs` (module declaration)
- `libtonode-tests/tests/tip_spend_rejection.rs` (suite rewiring)
- `Makefile.toml` (drop the hardcoded nextest `--verbose`; user-ratified
  2026-07-16)
- `libtonode-tests/tests/mempool_attribution.rs` (dated correction of
  the boundary_rejection_attribution cross-reference)
