# A part's anchor boundary is drawn at an age of one bucket or more, never at the window it broadcasts in

Status: accepted. A ZIP 318 conformance fix, not a relaxation. Scoped to Phase 2
(scheduled parts). Supersedes the second decision of ADR 0017, that
`plan_schedule` opens the first cohort in the current bucket; that ADR's first
decision, a part being due for its whole open window with an advisory target,
is untouched.

A scheduled part carries an **anchor bucket distinct from the bucket it
broadcasts in**. The anchor is `bucket_index - age`, where `age` is drawn
`Geometric(1/2)` truncated to `[1, ANCHOR_AGE_CAP]` (the cap is 16 buckets) and
reject-sampled against the part's own candidate set: the anchor bucket must sit
strictly above the NU6.3 activation bucket and at or after the boundary at or
after the confirmation height of the part's bound note. Since the smallest
legal age is one, a part's anchor is always a boundary the chain has already
left, and the broadcast window is at least one full bucket (`M` = 144 blocks,
~3h at the 75-second target spacing) above it. The transaction's
`target_height`, which selects the consensus branch ID and seeds the canonical
expiry, is now derived from the **broadcast window** (`boundary_of(bucket_index)
+ 1`) rather than from the anchor, so the two heights no longer move as one.

Before this change a part assigned to bucket `i` both broadcast while the tip
was inside bucket `i` and anchored to the tree state at boundary `i·M`, so its
anchor was always the most recent boundary at broadcast time: age zero, every
time. The conformance oracle, `zcash_pool_migration::scheduling::draw_anchor_boundary`
(pinned at `=0.1.0-alpha.1`, see `zingolib/Cargo.toml:105`), builds its
candidate set from the boundaries simultaneously strictly above the NU6.3
activation height, at or after the funding note's creation height, and at or
before `most_recent_boundary(chain_tip) - BOUNDARY_MODULUS` — that last
condition being exactly the requirement that the anchor lie a full bucket in
the past. Upstream documents the geometric draw as the ZIP 318
ANCHOR-AGE-DRAW MUST and states outright that age zero is never produced. The
reason is anonymity-set size: an age-zero anchor is the newest tree state in
existence, so the cohort proving against it — the transfers network-wide that
share the anchor — has not accumulated yet, and a part that anchors there
proves against a set of near-nothing. Our parts violated that MUST by
construction, because anchor and window were the same bucket.

Decoupling also relocates the note-confirmation floor. Under the old shape the
floor was a maximum taken over every bound note in the schedule, since one
boundary served as both anchor and window; now each part's anchor is drawn
against its own note, which buys anchor diversity within a batch. It buys no
window latency: a broadcast window is shared by every part of the batch
assigned to it, so the window's floor stays the batch-wide maximum.

## Considered Options

**Ratify age zero as a knowing deviation**, the way ADR 0017 ratified the
advisory target. It is the cheapest option and changes no code, but it ships a
stated MUST violation whose whole cost is privacy: our parts would prove
against a materially smaller anchor anonymity set than the specification builds,
and unlike the advisory target there is no responsiveness win on the other side
of the trade. Rejected.

**Adopt `zcash_pool_migration::scheduling` wholesale**, taking its shuffle,
exponential inter-arrival delays, and geometric anchor draw together. That
buys conformance by construction and retires our scheduler, but it drops
`k_max`, `target_sessions`, and consecutive-window batching, which exist to
bound how many signing sessions a mobile user must sit through. Rejected: we
adopt the anchor draw and keep the batching. The crate stays a dev-dependency
oracle, so the conformance constants can never move under the consent hash via
a dependency bump.

A third, smaller relaxation was considered and declined earlier on its own
merits: loosening the activation floor from "boundary at or above activation"
to "window not entirely pre-activation", by computing `target_height` as
`max(boundary + 1, activation)`. The win was at most 143 blocks, once, and only
for wallets scheduling before activation. Decoupling then rendered it moot,
since the target no longer sits on the anchor boundary at all.

## Consequences

For a wallet that has **just** split, the first batch lands one bucket (~3h)
later than before: a fresh note's anchor floor is the next boundary, and a legal
window must then sit one bucket further still. A wallet whose notes confirmed at
least a bucket ago pays nothing — the current bucket remains a legal window,
with an age-one anchor beneath it — so ADR 0017's responsiveness win survives
for every wallet that is not scheduling in the same breath as its split.

The activation floor on the *window* becomes redundant and is no longer the
thing that binds. Legal anchors already sit strictly above the activation
bucket and every window sits at least one bucket above its anchor, so a window
clears activation automatically. This also corrects the reason recorded around
the old floor: what forbade a pre-activation boundary was never that it could
not anchor an Ironwood output (a historical Orchard root is a perfectly legal
anchor, and `ironwood_anchor` is the empty tree), but that `target_height` was
derived from that boundary, and the target selects the consensus branch ID,
which decides whether an Ironwood bundle may exist at all. See ADR 0014.

The anchor now sits at least a full bucket before the window, so eager witness
capture (`refresh_part_witnesses`) has a bucket's worth of syncs to catch the
anchor's checkpoint before the part is due, where previously the window opened
on the same boundary it needed witnessed. That runway is what makes it
survivable that pepper-sync retains no checkpoints on the boundary grid: a
missed capture has time to be retried rather than immediately stranding a part.

The migration section's own inner version goes 3 → 4
(`zingolib/src/wallet/migration/store.rs`); `LightWallet::serialized_version()`
stays 42, since the section carries its version independently. A legacy part
still unsigned is read back anchorless, and the next placement or the next
`refresh_part_witnesses` pass draws it a legal age; its cached witness is
discarded along with its anchor, because that witness proves the note under the
*window's* boundary and surviving into a redrawn anchor would either fail to
prove or quietly resurrect the age-zero anchor this decision retires. A part
already `Signed` keeps the age-zero anchor its signature commits to, witness
included, and broadcasts it once, because re-anchoring would invalidate the
signature and re-signing would demand another user visit. `params_hash` and `MigrationParams` are untouched, so
existing consent stays valid and no user is asked to consent again;
`ANCHOR_AGE_CAP` is a local constant, deliberately not a consent-hashed
parameter, since it governs how a part is scheduled rather than what the user
agreed to migrate.
