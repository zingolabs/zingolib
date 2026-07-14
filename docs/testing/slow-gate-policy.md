# The slow gate: runtime outliers leave the default suite

Repository-wide policy, set 2026-07-08 by user direction: any test whose
runtime exceeds **2× the median runtime of its crate's default suite**
is gated behind a non-default `slow` cargo feature. The default suite
stays tight enough to run habitually; the outliers remain one switch
away (`--features slow`, or `extra-credit-tests` where a crate
aggregates its non-default gates).

## Metric

The measure is **steady-state solo wall-clock** as recorded by nextest
in the most recent full-suite run of that crate: the time a test takes
alone on a warm machine, chain caches populated. One-time costs that
the chain-cache framework amortizes (a first-run cache build, a
`ZINGO_REGENERATE_CHAIN_CACHE` rebuild) are excluded — they are paid
once per machine, not per run, and have their own budget mechanism
(per-test nextest `slow-timeout` overrides). Parallelism stretch under
container contention is likewise excluded; it distorts every test
equally and is governed by the `local-net` test group's thread cap.

## Procedure

At each suite re-baseline (or whenever a full-suite run's timings are
recorded):

1. Take the per-test wall-clock times for the crate's default suite.
2. Compute the median; the threshold is twice that.
3. Move every test above the threshold behind the crate's `slow`
   feature, and bring back any gated test that has fallen below it —
   the gate reflects current measurements, not history.
4. Record the median, threshold, and resulting membership in this
   file's log below.

A test whose slowness is *the point* — a sentinel pinning an
environment contract, a cache builder — belongs behind its
purpose-specific gate (`sentinels`, ignore-with-reason), not the slow
gate; the slow gate is for ordinary coverage that happens to be
expensive.

## Wiring

`libtonode-tests` declares the empty `slow = []` feature, aggregated
into `extra-credit-tests`. Other test crates adopt the same shape when
their first outlier appears. Gate an entire file with
`#![cfg(feature = "slow")]`, or a single test with
`#[cfg(feature = "slow")]` on the test function.

## Classification log

- 2026-07-08: gate established, membership empty. No recorded full-suite
  timings were available on the host at policy time (nextest stores are
  in the container volume); the first classification executes against
  the next recorded full-suite run. Known solo datapoints for context:
  replayed chain-bound tests run ~17–30 s; the checkpoint-window test's
  steady-state (replay) arm is ~24 s — likely within threshold, its
  8.5-minute first-run build arm being excluded as a one-time cost.
