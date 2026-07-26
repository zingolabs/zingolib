# ZIP 318 divergence ledger

The adjudication record ADR 0020's behavioral-idempotency requirement
demands: every deliberate divergence between this wallet's migration
behavior and the canonical `zcash_pool_migration` implementation, with
its status and its unblocking dependency where one exists. A divergence
absent from this ledger is a defect. The `zip318_conformance_tripwires`
suite enforces the value layer; this ledger records the behavior layer.

## Adopted (conformant as of params version 2)

The preparation bound moved from at most 32 actions to the ZIP's
16-action preparation shape
(<https://zips.z.cash/zip-0318#notepreparationtransactions>).

The advisory target-draw law moved from uniform-in-window to the
canonical exponential inter-arrival distribution, mean 144 blocks capped
at 576, drawn from `SchedulingParams::ZIP_318`
(<https://zips.z.cash/zip-0318#transferscheduling>). One gap remains
open within it: each target is drawn independently from its window
boundary, while the canonical law chains successive transfers (each
delay from the previous transfer). Chaining arrives with the scheduling
delegation deferred under ADR 0020's standing pull.

## Blocked (divergent, dependency named)

The part transfer's Ironwood bundle is padded to two actions and its fee
is 20 000 zatoshis, against the canonical single unpadded Ironwood
action and 15 000
(<https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>).
The registry `zcash_primitives` 0.29 builder exposes no Ironwood padding
control; `BuildConfig::Standard` gains `ironwood_padding: BundlePadding`
in 0.30. The divergence unblocks with the librustzcash stack bump to the
0.30 line, which is also what the deferred PCZT builders require. The
fee change carries its own `MigrationParams` version bump when it lands.

## Retained local (the ZIP standardizes no value)

`sweep_min` (twice the ZIP 317 marginal fee): the ZIP defers small-note
economics to ZIP 317
(<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).

`k_max` and `target_sessions`: the ZIP names `K_MAX` without fixing a
value (<https://zips.z.cash/zip-0318#whalehandling>), and the signing
session target is wallet ergonomics.

Whole-open-window sendability (ADR 0017): a client policy layered over
the canonical schedule; the drawn target stays advisory.
