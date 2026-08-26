# Server selection is a mixnet liveness sweep

Status: draft — ratified in session 2026-08-06, pending review

## Context

The clearnet latency race that once selected the sync server is gone.
Its census had lapsed, its probes sprayed `get_info()` from the user's
real IP at whoever now holds those domains, and the 2026-08-06 rulings
made both defects structural: a session's only clearnet communication
is sync itself, and sync may attach only to an indexer whose liveness
was established over the mixnet. The race's code survives solely under
the non-default `clearnet-test-mode` feature, and re-incorporating any
of it requires explicit review.

That left server selection with no implementation. The wallet already
reaches indexers over the mixnet in-process — send, price-fetch, and
the mixnet probe all ride the session's SOCKS5 transport — so the
replacement needs no new transport machinery. ADR 0029's Maintained
Indexer Pool describes the destination (a maintained, per-exit-probed
pool), but its prototype is unwired and carries confirmed defects, and
sync needs a selection mechanism now.

## Decision

Server selection is the Server-Selection Sweep, owned by zingolib and
run entirely over the mixnet. Every Sync Session opens with one; no
other event triggers selection.

The sweep sends one `GetLightdInfo` per candidate through the session's
mixnet transport. The candidates are the census's active
mixnet-eligible entries for the session's chain — `https` on port 443,
the one endpoint shape the exit policy carries — together with any
candidates the user supplies. A candidate is live when its reported
chain matches the session's chain and its reported height sits within
two blocks of the sweep's observed median height. The median defends
the cohort: a single indexer advertising an inflated height cannot
capture selection, where a maximum-height sort would hand every wallet
to the best liar.

Height qualifies; the draw selects. Within the live cohort the ratified
sync-attach rule applies unchanged: a uniform random draw over the live
Indexer Operators, one ticket per operator, any live endpoint of the
winner, sticky for the Sync Session. The height-descending order is the
failover sequence and the report order, never the selector.

An explicit user pin bypasses the draw, never the sweep. The pinned
server is surveyed like any candidate and judged against the same
cohort median. When it is live it is selected. When it is not, the
session drops to the liftable offline posture and names the dead pin
as the reason; the in-session consent act retries the same pin, and
nothing ever substitutes an unpinned choice.

An empty cohort without a pin ends the Sync Session with a typed
refusal that names every candidate's failure. The session's posture is
untouched: the mixnet stands, and send and price-fetch continue. A pin
binds the session; an unpinned sweep binds only its Sync Session.

The sweep rides its own dedicated transport. Its Exit Node is never the
one carrying send or price-fetch — before or after the sweep — and it
is recycled the moment the sweep completes: the survey's transport is
torn down, and the transmit transport never learns what was surveyed.
The selected sync indexer is excluded from the live candidates that
serve the transmit operations, so the operator that sees the wallet's
sync stream is never also a transmit target.

## Consequences

Sync gains a mixnet round trip per candidate at every Sync Session
start, and the recycling adds one bootstrap before the next
transmission. Both costs buy the two invariants: no clearnet contact
outside sync itself, and no single exit that links the survey to the
wallet's later traffic.

Latency leaves the selection vocabulary. Over per-exit mixnet paths,
cross-candidate latencies measure the exits, not the indexers, so
height is the only quality signal the sweep trusts.

The census becomes the selection's sole candidate source, which
commits the project to keeping it current and to collapsing zingolib's
separate broadcast-indexer literal into it.

ADR 0029 is amended, not replaced: the Maintained Indexer Pool, when
it lands, becomes the sweep's maintained successor — the same
qualification and draw over a pool whose liveness is maintained by
per-exit probes instead of re-established each Sync Session.

## Addendum (2026-08-08)

ADR 0029 is superseded, so the maintained-successor consequence above
is void: the sweep's successor is the Destination Pools with the
transmissions-are-probes Health economy. The sweep itself gains a
ratified second role: it is the session's baseline health probe, the
last probe-only act a session performs. The judgment this record calls
liveness is since named Health in the glossary.
