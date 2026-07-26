# Migration logic delegates to zcash_pool_migration

Status: accepted 2026-07-27. Supersedes ADR 0018's "the crate stays a
dev-dependency oracle" clause and its rejection of wholesale scheduling
adoption, ADR 0017's locally-implemented window scheduler, and the
internals (never the API) of ADR 0016's note splitting. The consumer API
of zingo-cli, zingo-pc, and zingo-mobile is a hard constraint and does
not change.

We delete the bespoke ZIP 318 implementation wherever the canonical
`zcash_pool_migration` crate (the reference implementation, in
zcash/librustzcash) provides the logic, and we delegate to it. The
conformance this buys is a security property and a NON-NEGOTIABLE
REQUIREMENT: denominations, transaction shapes, and schedules exist so
that every migrating wallet's behavior collides with the population's,
and a wallet running bespoke variants of them fingerprints its users.
zingolib keeps its public types and `LightClient` methods as the stable
consumer surface, converting to and from upstream types at one boundary.

Concretely, planning and quantization delegate to `plan_note_split` and
`CanonicalOneTwoFive` (the strategy is a pure function of the balance,
so preview-equals-execute and the consent `plan_hash` survive; the
trait's RNG parameter is an affordance the canonical strategy ignores).
The preparation model delegates, adopting the 16-action preparation
transaction of
<https://zips.z.cash/zip-0318#notepreparationtransactions>, with the
signing-session batching that ADR 0018 preserved expressed through the
planner's `prep_tx_count` callback rather than a parallel planner.
Transfer scheduling delegates to the upstream schedule and anchor
machinery, retiring the local bucket-window scheduler. The engine and
state machine delegate through the crate's `PoolMigrationRead` and
`PoolMigrationWrite` storage traits, which the wallet implements over
its own file: the upstream `MigrationState` persists as wallet format
revision 44, landed together with a permanent tolerant reader for the
shipped bespoke layout and its regression tests, per the wallet-format
ADR. Transaction building phases in: pool-crossing transfers adopt
`build_transfer_pczt` first (realizing the PCZT convergence ADR 0008
recorded and issue #2425 tracks), while preparation transactions keep
the local builder until the PCZT prove-sign-finalize flow has soaked.
The immediate Drain stays local: upstream implements no immediate path,
and the Drain's send-shaped API (ADR 0019) is consumer surface.

The dependency points at the canonical source, not at a release: a git
dependency on the default branch of zcash/librustzcash, floating. This
is a ratified exception to the no-new-dependencies/no-pins rule, and it
deliberately supersedes the exact-version-pin rationale recorded when
the crate was first imported. Three guards replace the pin. The
`zip318_conformance_tripwires` suite pins every imported value to the
ZIP's literal number with a hash-fragment citation, so upstream moving a
ratified value fails red. A branch-move tripwire reads the workspace
`Cargo.lock` and asserts the resolved `zcash_pool_migration` revision
against a pinned literal, so every float of the default branch turns
exactly one test red and is adopted by a deliberate commit that
re-adjudicates the value tripwires. And the behavioral ledger below
re-adjudicates observable behavior. The known hazard is sibling drag: a
git dependency from the librustzcash workspace can pull git versions of
its sibling crates into resolution, and each adoption commit owns that
fallout.

Behavioral idempotency is demonstrated by twins with a divergence
ledger, on the pattern of `docs/testing/live-offline-twins.md`. Before
the swap, golden vectors captured from the bespoke implementation (plans
from fixed note sets, schedules from seeded draws, fees, state
round-trips) and a property suite of the invariants (value conservation,
residual bounds, denomination membership, replan idempotence,
preview-equals-execute) pin current behavior. After the swap the
invariant suite must pass unchanged, and every golden divergence is
adjudicated in a recorded table. The ratified divergences are known in
advance: the part fee moves from the bespoke four-action shape to the
canonical two-source-one-destination shape, preparation transactions
move from at most 32 actions to exactly 16, the schedule law moves from
bucket windows to the upstream draws, and the state serialization moves
to format 44. `MigrationParams` bumps its version, so every one of these
lands as a detectable consent change.

No in-flight migration machinery is built: this policy ships before
migration starts, so no production wallet holds consented bespoke-flow
state. A dev-built wallet carrying the shipped bespoke migration section
is read tolerantly and retired to fresh consent.

## Considered Options

Keeping the bespoke implementation with the crate as a dev-dependency
conformance oracle was the previous posture (ADR 0018). It was rejected
because hand-maintained conformance across planning, scheduling, fees,
and shapes is a standing drift risk on the exact surfaces where drift
fingerprints users, and the oracle pattern had already let the fee and
preparation shapes diverge three ways from the ZIP's prose.

Delegating everything at once, including preparation building, was
rejected in favor of transfers-first phasing: the pool-crossing transfer
is where canonical shape matters most for the anonymity set, and the
PCZT flow is new machinery best soaked on one path before it carries
two.

A rev-pinned git dependency was considered as a middle ground preserving
the no-silent-movement property at the dependency layer. The floating
default branch was chosen deliberately: zingolib does not ship a crate,
canonical logic is wanted at merge cadence, and the branch-move tripwire
restores the loud-movement property at the test layer instead.
