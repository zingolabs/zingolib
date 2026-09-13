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

## Amendment (2026-09-11): per chain, on every transport, from the registry

Three corrections, ruled with ADR 0050.

What the sync indexer holds. The context above says "the wallet's full
address set". Shielded addresses never leave the wallet: compact blocks
come down in ranges and trial decryption is local. What the sync indexer
learns is every transparent address, sent in the clear by
`get_taddress_txids` and `get_address_utxos`; every txid the wallet owns a
shielded note in, because the wallet fetches the full transaction by txid
after a trial-decryption hit; and the client IP on clearnet sync. The
invariant stands on that linkage, and it is enough: the operator holding
the owned-txid set must not also receive the raw transaction.

The clearnet exemption is revoked. Toggling the mixnet off is consent to
expose the client IP to a Destination; it is not consent to hand the
broadcast to the sync operator. A switched-off session draws through the
same set as a ready one and races the same escalation over direct
connections. Diagnostic traffic carrying no wallet data stays exempt.

The invariant is a chain policy, and the pool is the Destination Server
set. The sanctioned constructor is `DestinationServerSet::draw`, which
reads the registry partitioned to the session's chain and applies the
exclusion at draw time against the current sync indexer. Mainnet keeps
`ExcludeSyncOperator` exactly as decided above. Testnet draws under
`IncludeSyncIndexer` and regtest under `SyncIndexerOnly`: those chains carry
no value, and their registries hold no operator diversity to rotate over,
so the exclusion there could only refuse. A trusted server the user
vouches for is drawn alone on any chain; that half of the set is wired in
code and not yet fed by configuration (ADR 0050, amendment 2026-09-12). The hand-curated
`DESTINATION_INDEXERS` and `eligible_witnesses` are retired.
