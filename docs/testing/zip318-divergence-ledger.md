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
into bucket space with the transfer's window boundary playing the observed
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

The transfer's Ironwood bundle is padded to two actions and its fee
is 20 000 zatoshis, against the canonical single unpadded Ironwood
action and 15 000
(<https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>).
The registry `zcash_primitives` 0.29 builder exposes no Ironwood padding
control; `BuildConfig::Standard` gains `ironwood_padding: BundlePadding`
in 0.30. The divergence unblocks with the librustzcash stack bump to the
0.30 line, which is also what the deferred PCZT builders require. The
fee change carries its own `MigrationParams` version bump when it lands.

Transfer ordering is largest-denomination-first and deterministic:
`plan_schedule` ranks the quantized transfers so the largest land in the
earliest windows, while the ZIP requires a uniformly random shuffle and
names largest-first as its counterexample — the ordering lets an
observer infer migration progress from any transfer it can attribute, and
makes the sequence predictable to a targeted adversary who knows the
balance (<https://zips.z.cash/zip-0318#transferscheduling>). The batch
scheduler consumes the ranking, so the shuffle cannot be dropped in
without it; it arrives with the scheduling delegation to the upstream
`schedule` machinery (which shuffles internally), deferred under ADR
0020's standing pull.

Preparation broadcasts are not temporally decoupled: preparation rounds
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

## Deliberate (ratified 2026-09-17)

The reservation is hard. The ZIP assumes the user may spend Orchard funds
outside the migration and requires the wallet to detect the spend and
rebuild the schedule
(<https://zips.z.cash/zip-0318#errorhandling>). This wallet never selects
a reserved note for an ordinary send: the proposal fails with the reserved
amount, and the user releases a transfer or cancels to spend it. The
detect-and-rebuild path stays, because another device with the same seed
can still spend the note. Chosen because a spent funding note breaks the
committed plan, leaves off-denomination change, and costs more
preparation transactions.

A submitted transfer that missed its window is discarded and re-signed
one window after its own closed, when the scanned chain shows its note
unspent. The ZIP says a stored signed transaction "MUST NOT be discarded"
on a transient failure and offers the user "send now or retry in the
background". This wallet keeps and retries within the window, then
treats the transaction as lost. The old transaction can still mine later;
then two transactions spend one note, one confirms, and reconciliation
counts either as this transfer's own (`previous_txids`). Chosen so a lost
transaction waits hours, not the 30 to 60 days of the canonical expiry.
A signed transfer that never left the device is re-signed at once.

A missed window is rescheduled automatically into a later window, with
the miss counted on the transfer, and the explicit "send now"
(`broadcast_missed_now`) stays for the on-launch prompt the ZIP
requires. The ZIP's "retry in the background" is the default; the count
lets the consumer decide when to prompt.

## Retained local (the ZIP standardizes no value)

`sweep_min` (twice the ZIP 317 marginal fee): the ZIP defers small-note
economics to ZIP 317
(<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).

`k_max` and `target_sessions`: the ZIP names `K_MAX` without fixing a
value (<https://zips.z.cash/zip-0318#whalehandling>), and the signing
session target is wallet ergonomics.

The immediate migration's chunk bound: the ZIP standardizes no shape
for the non-private option and upstream implements no immediate path
(ADR 0020), so the bound is local policy. Each immediate transaction's
spends beside its single Ironwood output fit the same 16-action total
budget the preparation transactions carry, through the same side-budget
law, so every migration transaction the wallet emits shares one shape
family; the fee a larger chunk would save is small even on a very
fragmented wallet. Previously an independent 32-input cap.

Whole-open-window sendability (ADR 0017): a client policy layered over
the canonical schedule; the drawn target stays advisory.
