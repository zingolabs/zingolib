# A platform-typed Mixnet Session acquires every transport

Status: draft — ratified in session 2026-08-10, pending review

## Context

The 2026-08-10 audit of the mobile send path found that the invariants
of ADR 0038 and ADR 0039 are enforced only on desktop, because the
mobile consumer enters the mixnet through a different door. A spawned
session acquires every transport through the acquisition seam, which
draws each exit through the Exit Pool ledger; an attached session
calls the plain attach entry, which accepts one host-chosen, unledgered
tunnel that every surface then shares. Three further seams trusted the
platform rather than the seam: the send and price paths gated pooling
on whether the transport was a spawned child, the Server-Selection
Sweep constructed the desktop acquirer directly, and the provisioning
strategy could not express host-serviced acquisition at all. The seam
itself was a trait object, and its vocabulary was stringly: exit
identities, proxy addresses, responsiveness classes, and host errors
all crossed it as bare strings.

## Decision

The acquirer is the only door. Every mixnet transport a session uses
is acquired through its one acquisition seam, so every exit passes the
Exit Pool ledger and the unique-per-holder Reservation invariant holds
on every platform by construction. The consumer-facing attach entry
that admits an unledgered tunnel is removed.

Dispatch across platforms is compile-time checked, never a trait
object. A sealed platform marker type — one for the desktop child
process, one for the app-serviced mobile host (marker names are
working identifiers pending ratification) — carries the concrete
acquirer as an associated type. The generic is confined to the
**Mixnet Session** (name ratified 2026-08-10): the one subsystem
struct owning the session's transport slot, Destination Pools, Exit
Pool, and acquirer. The wallet session holds a Mixnet Session typed by
its platform, with the desktop platform as the default parameter so
existing desktop consumers compile unchanged.

The mobile acquirer is a concrete channel: the wallet sends plain-data
requests — discover the exit population; start a transport under a
responsiveness class over a Clutch — and the app services them,
calling its UniFFI callback on its own thread. The one unavoidable
vtable, the UniFFI callback interface, stays inside the mobile FFI
crate, outside this workspace.

The seam's vocabulary is typed. Exit identities travel as an exit-node
identity type, proxy endpoints as socket addresses, the responsiveness
class as its existing enum, and host failures as a typed refusal.
Strings survive only at the true FFI edge, converted at the boundary.

## Considered options

A trait-object seam (the prior shape) was rejected because it proves
nothing at compile time and was ruled out in session. An enum acquirer
was rejected because it ships both platforms' acquisition code in
every binary, leaving the mobile no-subprocess rule a convention. An
unconfined platform parameter on the wallet session type was rejected
because it spreads a generic through every consumer signature that the
confined Mixnet Session keeps internal.

## Consequences

A mobile build provably contains no process-spawning code: the
sandbox constraint of ADR 0011's mobile amendments becomes a
compiler-checked property rather than a convention. The runtime
spawned-versus-attached gates on the send and price paths dissolve,
because pooling capability is a property of the platform type. The
Server-Selection Sweep and the Price Source Pool reach mobile through
the same seam, which is what extends pairwise-disjoint Exit Node
Reservations to every concurrent surface there. zingo-mobile's shim
must implement the host service for the request channel. Destination
disjointness across concurrent Transmissions remains unenforced on
every platform and is a separate future decision.
