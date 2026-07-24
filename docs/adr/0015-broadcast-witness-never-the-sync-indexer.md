# A Broadcast Witness is never the sync indexer

A transmission draw excludes the sync indexer's operator from the witness
pool, universally: any surface, present or future, that hands a raw
transaction to a drawn indexer must draw from a pool that cannot contain the
party the wallet synchronizes against. The rule is enforced in code at the
single pool constructor, not left as a convention, and an exclusion that
empties the pool refuses the send rather than falling back.

## Context

ADR 0011 names the indexer as the adversary and builds Witness Rotation
around it: each send goes to one randomly drawn Broadcast Indexer so that no
single operator accumulates a picture of the wallet's sends. But the sync
indexer is not just any operator. It already holds the wallet's full address
set from serving sync queries, and under bare-clearnet sync it holds the
client's real IP as well. A witness draw that lands on the sync indexer
therefore hands the one party with the address book the raw transaction too —
the maximal linkage the whole mixnet arc exists to prevent — and it does so
silently, on a random schedule, with nothing in the design forbidding it.

The overlap is not hypothetical. The sync server is user-configurable and
defaults to the same well-known operators the curated Broadcast Indexer list
draws from; with the default configuration, roughly one first-round draw in
eleven went to the sync operator before this decision.

## Decision

The invariant: **a Broadcast Witness is never the sync indexer.**

Enforcement is structural. The curated list's module exposes one sanctioned
pool constructor for transmission draws, `eligible_witnesses(sync_indexer)`,
and the fan-out obtains its candidates only through it. The raw curated list
remains available to surfaces that carry no wallet data, and its
documentation forbids transmission use.

Matching is by operator, not by exact URI, for the same reason the curated
list holds one endpoint per operator: the accumulating party is the operator,
so a sync connection to `eu.zec.rocks` excludes the `zec.rocks` witness. The
operator key is the registrable parent domain, approximated as the host's
last two labels. The approximation errs only toward over-exclusion — two
unrelated hosts sharing a suffix collapse into one key and both leave the
pool — which shrinks the pool slightly but can never let the sync operator
through.

The invariant fails closed. If exclusion empties the pool — impossible with
the current eleven-operator list, but reachable should the list ever shrink —
the send refuses with a typed error naming the invariant, in keeping with ADR
0011's rule that a mixnet send never silently degrades.

The invariant is universal across deployments and surfaces. It binds the
desktop spawned-proxy path and the mobile attached-endpoint path identically,
because enforcement sits in the draw, upstream of any transport. It binds
every future broadcast surface, including the reserved seat for a group
broadcast service: a new surface that draws a transmission target must draw
through the sanctioned constructor.

## Boundaries

Two adjacent paths are deliberately outside the invariant, recorded here so
their exemption reads as a decision rather than an oversight.

Diagnostic and health traffic carrying no wallet data is exempt. The `nym
probe` pairing intentionally probes the full curated list — measuring a
witness is not broadcasting through it — and the proxy's readiness gate sends
a bare `GetLightdInfo` round trip that carries no transaction and no address.

The clearnet consent path is exempt. When the user deliberately toggles the
mixnet off, the send is a direct submission to the configured indexer under
ADR 0011's informed-consent rule; no draw occurs, so no witness exists to
constrain. The user who chose clearnet chose to trust the sync indexer with
the broadcast, and this decision does not reverse that ratified consent.

## Consequences

With the default configuration the witness pool shrinks from eleven operators
to ten, which still comfortably feeds the fan-out's cap of six distinct
witnesses. A user who syncs against a private indexer excludes nothing and
draws from the full pool. Regression tests pin the operator-level exclusion,
the untouched pool for an out-of-list sync server, and the fail-closed refusal
on an emptied pool.
