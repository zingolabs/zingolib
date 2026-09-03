# An Exit Node is Exclusive to one Destination or Shared across many

Status: draft — ratified in session 2026-08-07, pending review.
Amended 2026-08-10: the sealed categories are implemented as lease
types — `Member<T, Exclusive>`'s one dial consumes the member, and
`Member<T, Shared>` dials repeatedly for its one holder — declared at
the Indexer Pool, Price Source Pool, and Server-Selection Sweep
acquisition sites. The session slot's tunnel crosses the route as
`SlotTunnel`, which names the Shared category without enforcing it:
the tunnel is a plain address carrier, not a lease, and the
one-exit-many-requests discipline rests on the slot supervisor.

## Context

Every transmitting surface reaches its Destinations through a mixnet
Exit Node drawn from the Exit Pool, but the surfaces differ in what the
bound exit gets to observe. A Transmission's connection dials exactly
one Destination, so its exit learns a single (client, Destination)
pairing. The Server-Selection Sweep and the price fetch deliberately
fan out to many Destinations through one exit, so that exit observes
the whole fan-out set — an exposure those operations accept because
their traffic names no wallet.

The 2026-08-07 session found this distinction latent and unenforced:
nothing stopped a future path from dialing a second Destination
through an exit that a send had bound. The session also required, as a
condition of the design, that the distinction be carried at the type
level and enforced at compile time where feasible, in the shape the
responsiveness partition already established.

## Decision

Exit-node use is partitioned into two sealed categories. An
**Exclusive Exit Node** serves exactly one Destination for the whole
life of its holder's Exclusive Lease. A **Shared Exit Node** serves
many Destinations for its one holder. The category is a property of
the acquiring operation, declared at the acquisition site and carried
by the lease type; the Exit Pool stays homogeneous, and no node carries
a category of its own.

Enforcement is structural. The exclusive lease's connect operation
consumes the lease, so a second dial through the same exit is
unrepresentable; the shared lease permits repeated dialing. A send arm
holding a shared lease, or a sweep iterating over consuming leases,
fails to type-check rather than failing in review.

Shared never crosses holders. The Exit Node Reservation's
unique-per-holder invariant (ADR 0038) is untouched: a Shared Exit
Node is exclusively held, and its sharing is among the holder's own
Destination connections only.

The partition is orthogonal to the responsiveness partition. The
Server-Selection Sweep races its acquisition at full width yet binds a
Shared exit; a send arm hedges its acquisition yet binds an Exclusive
exit. The two classifications therefore travel as two independent
type parameters at the acquisition site, never as one fused four-way
class.

## Consequences

The send path's Destination-exposure property becomes structural:
the exit an arm binds cannot observe a second Destination, whatever
the escalation above it does. The fan-out surfaces name their exposure
explicitly by declaring Shared, making the accepted linkage auditable
at the acquisition site.

The ruled change of send's race discipline — hedged full-path arms in
the RFC 8305 style, with an interval long enough that a responsive
Destination wins before a second arm launches — builds on this
partition and was decided the same day as ADR 0040, which replaces
the serially gated rounds of ADR 0011.

Implementation follows the reservation machinery this partition
presupposes; neither the pool nor the leases exist in code yet, so
this record governs their design rather than amending shipped
behavior.
