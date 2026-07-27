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

## Delegated (Landing C: the mirrors are deleted)

The anchor draw delegates to `scheduling::draw_anchor_boundary`, mapped
into bucket space with the part's window boundary playing the observed
chain tip; the local geometric age draw and rejection loop are deleted,
and seeded golden vectors captured from the retired mirror pin the
equivalence
(<https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>).

The expiry computation delegates to `scheduling::expiry_height`; the
local arithmetic is deleted, and the tripwire pins the delegated result
to the ZIP's own arithmetic over a sample of heights
(<https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>).

The decomposition mirror is retained for now: the canonical crate
exports the `{1, 2, 5}` series only inside its `DenominationStrategy`
planning entry points, no bare decomposition, so `quantize::decompose`
keeps the ladder walk under its shape and conservation proptests. Its
delegation arrives with the planning-layer delegation.

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

Part ordering is largest-denomination-first and deterministic:
`plan_schedule` ranks the quantized parts so the largest land in the
earliest windows, while the ZIP requires a uniformly random shuffle and
names largest-first as its counterexample — the ordering lets an
observer infer migration progress from any part it can attribute, and
makes the sequence predictable to a targeted adversary who knows the
balance (<https://zips.z.cash/zip-0318#transferscheduling>). The batch
scheduler consumes the ranking, so the shuffle cannot be dropped in
without it; it arrives with the scheduling delegation to the upstream
`schedule` machinery (which shuffles internally), deferred under ADR
0020's standing pull.

Preparation broadcasts are not temporally decoupled: splitting rounds
broadcast back-to-back, each round as soon as the previous one confirms,
while the canonical law spaces preparation broadcasts by exponential
delays with mean `PREP_MEAN_DELAY` (24 blocks) capped at
`PREP_MAX_DELAY` (96), precisely to keep a burst of identically shaped
padded transactions from forming a linkable cluster that also telegraphs
the coming schedule
(<https://zips.z.cash/zip-0318#notepreparationtransactions>). Upstream
exports the law (`draw_prep_delay`, `schedule_prep_broadcast_heights`);
adoption arrives with the scheduling delegation, deferred under ADR
0020's standing pull.

## Retained local (the ZIP standardizes no value)

`sweep_min` (twice the ZIP 317 marginal fee): the ZIP defers small-note
economics to ZIP 317
(<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).

`k_max` and `target_sessions`: the ZIP names `K_MAX` without fixing a
value (<https://zips.z.cash/zip-0318#whalehandling>), and the signing
session target is wallet ergonomics.

Whole-open-window sendability (ADR 0017): a client policy layered over
the canonical schedule; the drawn target stays advisory.
