# Destinations are drawn from the indexer registry per chain, on every transport

Status: draft — ruled in session 2026-09-11, pending review

## Context

A testnet send over the mixnet failed every time. The Destination draw
read a hand-curated list, `DESTINATION_INDEXERS`, that held only the
mainnet survivors of the 2026-07-21 discovery sweep, and neither the list
nor the draw took a chain. A testnet wallet built a testnet transaction and
raced it against six mainnet lightwalletd hosts, each of which rejected it
after the resilience policy's retries, so the send surfaced an
`AllFailed` escalation minutes later naming six mainnet hosts.

The defect was structural rather than a missing entry. Two lists described
"which indexers exist": the indexer registry in `zingo-netutils`, which
carries a chain per entry and feeds the Server-Selection Sweep, and the
Destination list in `zingolib`, a frozen snapshot with no chain that fed
the send. They had already drifted: two of the eleven Destination hosts
were absent from the registry. The draw fused the ADR 0022 exclusion with
its source, so a chain whose registry holds one operator (testnet holds
`zec.rocks` alone) could only refuse. And the rotation ran on one
transport: ADR 0022 exempted the clearnet path, which submitted straight
to the sync indexer with no draw at all.

## Decision

One session-scoped component, `DestinationServerSet` in
`zingolib::destination::servers`, owns the Destinations a session may
transmit to, and every transmitting surface draws through it.

The set is derived, never curated. It is built once when the session opens
from the registry's live entries for the wallet's chain, one endpoint per
operator, and it is never written to disk: it is a function of the
registry, the chain, and the sync indexer, so it follows every registry
update and puts nothing beside the wallet that the indexer history's
privacy contract forbids.

The chain fixes the set at construction and rules its policy. Mainnet
draws under `ExcludeSyncOperator`, ADR 0022 unchanged. Testnet draws under
`IncludeSyncIndexer`: the adversary model is a mainnet concern and the
testnet registry has no operator diversity to rotate over, so the draw is
the registry and the sync indexer together, one entry per operator, and a
private testnet indexer is a Destination beside the public one. Regtest draws under
`SyncIndexerOnly`, because the registry lists nothing for it. A testnet
session therefore cannot name a mainnet host, by construction.

Reachability is a transport argument to the draw, not a property of the
set. A mixnet draw keeps https on port 443, the one shape the exit policy
carries; a clearnet draw keeps every member, port 9067 included. The
2026-07-21 finding that the exit gateways mishandle the lightwalletd port
is honoured where it applies and nowhere else.

The exclusion is applied at draw time against the session's current sync
indexer, not baked in at construction, so a session that rebinds its sync
indexer (the Server-Selection Sweep does) never draws against a stale
exclusion.

The clearnet exemption in ADR 0022 is revoked. The rotation is transport-
independent: a switched-off session draws through the same set and races
the same hedged escalation over direct gRPC connections. The `Wire` a send
travels chooses only how each arm's target is built, `GrpcIndexer` or
`Socks5Indexer`, and both run the one `resilient_transmit` policy under the
one race planner. Migration parts draw through the same set on both wires,
and the clearnet fallback to the synchronization endpoint with a logged
correlation warning is gone.

Supporting moves: the race orchestrator leaves the nym-gated mixnet module
for `destination::rotation`, since it now runs on clearnet; the Exit Pool
leaves `destination::pool` for `mixnet::pools`, since it was never about
Destinations; the registry gains the two hosts the hand list carried and
the registry lacked; failure attribution is renamed in engineering terms,
`charge_phase` and `FailurePhase` becoming `fault_domain` and `FaultDomain`
with `Unattributed` becoming `Unknown`; `TransmitRoute::Clearnet` names the accepting
Destination, and the CLI's transmit report renders `destination` on both
routes.

## Amendment (2026-09-12): a trusted set, wired in code and not yet used

Revoking the clearnet exemption has a cost for the self-hoster. A user
syncing against their own node with the mixnet off used to send through
that node and touch no public server; under the decision above the
clearnet draw excludes their node and races the transaction across public
operators, each learning the client IP and the raw transaction. That is a
regression for the users who took the most care.

The set therefore has two halves. The untrusted half is the registry, as
above. The trusted half is servers the user vouches for: a trusted server
may hold both the sync view and the broadcast because it is the user's
own party. A draw with a trusted server reachable over the wire goes
there alone, with no exclusion and no rotation, since rotation spreads
exposure across parties the user does not trust and buys nothing among
those they do. A trusted server that is reachable and dead is a typed
failure, never a fall-through to public operators. A trusted server the
wire cannot reach, a LAN node over the mixnet, does fall through: the
tunnel hides the client and the node never sees the send. Trust is
asserted by configuration and never inferred from registry membership; a
user pointed at an unlisted public server has not vouched for it. Regtest
is the degenerate case: its registry is empty, so its sync indexer is
implicitly trusted.

The code exists and is dormant. `DestinationServerSet::with_trusted`
populates the half and `draw` honours it, the unit tests and one mock-chain
test pin the branch, and no configuration feeds it: every production
session builds an empty trusted set, so the live draw is the untrusted
branch exactly as decided above. Wiring it means one field beside
`indexer_uri` in `ClientConfig`, a CLI flag, and a settings toggle in the
mobile app, each a separate change.

## Considered options

Keep the hand list and add a testnet list. Rejected: testnet has one
operator, so a second list would hold one entry and the ADR 0022 exclusion
would still empty it; and the drift between two hand lists is what caused
the defect.

Serialize the set beside the wallet. Rejected: it is derived state, a
snapshot would freeze a list that must follow the registry, and writing
the wallet's permitted broadcast targets to disk breaks the indexer
history's contract that nothing about where this wallet transmits survives
the process.

Fall back to the sync indexer when a mainnet draw refuses. Rejected: ADR
0011 rules that a send never silently degrades, and the refusal exists to
keep the transmission away from the party holding the wallet's
transparent addresses and owned-txid set.

## Consequences

The mainnet mixnet set is now every live registry operator on port 443,
which includes members the hand list had dropped as dead in July. A dead
member costs one hedge interval before the race widens past it, and the
Health floor retires it for the session after repeated failures. Ranking
the set by the Server-Selection Sweep's live evidence is the natural next
step and needs only a change to the draw's ordering.

Clearnet sends now spend a draw and may contact more than one Destination.
A drawn Destination on clearnet learns the client IP, so the privacy gain
is smaller than over the mixnet, but the sync operator no longer receives
the broadcast on any transport.

Mock-chain tests run on regtest, so their mixnet-attached sends draw the
mock indexer alone and the escalation's width is exercised by the
planner's own unit tests rather than by the mock chain.

Consumers of the CLI's transmit report read `destination` where the
clearnet route once said `indexer`.
