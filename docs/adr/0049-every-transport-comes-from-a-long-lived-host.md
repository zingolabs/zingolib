# Every transport comes from a long-lived host

Status: draft — ruled in session 2026-08-19, pending review and
implementation

## Context

The tree acquires transports two structurally different ways. `Acquirer`
is an enum over `Spawned` and `Hosted`, and the difference is not the wire
but the lifetime. `MixnetProxy::spawn` launches a `nym-proxy` child per
acquisition, hands it a clutch as launch arguments, and reads its bound
address and exit off stdout as `SOCKS5_ADDR=` and `NYM_EXIT=` lines. That
child serves one conduit and dies with it. `HostedProvider` asks a platform
host that was there before the request and remains after it, because a
mobile sandbox forbids subprocesses and the host owns the proxy.

One consequence blocks ADR 0047. That decision rules that the Exit Pool,
the health index, and failure attribution go below the seam, and that the
wallet stops naming exits. The pool cannot follow the exits into a desktop
child, because the child's lifetime is one conduit and the pool's value is
everything it has learned across many. So the state stays in zingolib's
`Pools`, and the wallet keeps naming exits — not because anyone chose that,
but because it is the only place on desktop that outlives a client.

The two platforms have therefore ended up with opposite halves of one
design. Desktop has the intelligence and cannot encapsulate it: `Pools`
draws a clutch weighted by the health index, issues Reservations so two
acquisitions cannot bind one exit, and convicts every drawn exit through
`condemns_drawn` when a bind fails. Mobile has the encapsulation and no
intelligence: `MixnetProxyHandle::start` calls `NymProxy::start`, whose
`draw_clutch` is a `seeded_shuffle` truncated to `RESERVATION_CLUTCH_SIZE`
over the whole discovered population, so a phone picks exits uniformly at
random on every birth and remembers nothing. Against a measured population
where roughly 30% of exits carry nothing, that is a materially worse draw.

`NymProxy` already anticipates both callers. `start_over(clutch)` serves a
parent that drew, and `start()` draws for itself "for a standalone run with
no parent to draw one". The mechanism is dual-mode; what is missing is a
single owner for the state that makes a draw intelligent.

## Decision

There is one acquisition model. Every transport is acquired from a host
that outlives the conduits it serves, through `ProxyHosting`. `Acquirer`
stops being an enum over two kinds of acquisition and becomes one path.

Desktop's `nym-proxy` becomes a long-lived, multi-conduit child that
implements that contract over a pipe. Mobile's component implements the
same contract over its UniFFI callback interface. The contract is
identical; only the wire beneath it differs, and that difference is forced
by the sandbox rather than chosen.

The Exit Pool, the health index, Reservations, and failure attribution move
into the host, which is now the only thing on either platform that outlives
a conduit. The wallet asks by role and receives a conduit, which is what
ADR 0046 and ADR 0047 require.

Cardinality stays a policy answer and is not unified. ADR 0045's four
role-bound clients on desktop and ADR 0048's single rotating client on
mobile are both preserved: one trait, two configurations, differing in what
`rotation_verdict` answers and how many conduits the wallet asks for.

## Considered options

**Keep two acquisition models and move `Pools` into `zingo-netutils` as a
library.** The cheapest path, and it satisfies ADR 0047's literal test,
since no wallet code would name an exit. Rejected because it leaves the
exits crossing to children as launch arguments, leaves two code paths to
maintain and test, and leaves mobile's draw unintelligent — the pool would
run in the wallet's process, which mobile's proxy cannot consult.

**Make desktop stateless, as mobile is today.** Uniform, and by far the
least work: delete the pool and let every proxy self-draw. Rejected on
measurement. With roughly 30% of exits carrying nothing, discarding the
health index makes every birth pay a coin flip the session has already
learned how to avoid.

**One long-lived host per role on desktop.** This keeps process isolation —
four processes, as today — while giving each the lifetime to survive client
rotation. Rejected because the pool would then be per-host, so four
processes would each learn separately and none would learn from the others.
Fragmenting the health index four ways is most of the cost of having none.

**One long-lived host, shared.** Chosen.

## Consequences

The fault isolation trade is the substance of this decision and deserves
stating plainly rather than as a footnote.

Today a desktop child is a containment boundary. One child dying costs one
conduit; the standing client's failover convicts the exit, births a
replacement, and the session dips to Bootstrapping and recovers. Four boot
clients cannot take each other down. Under one shared host, a crash costs
every conduit at once, and the session loses its transport entirely rather
than one quarter of it. The `DeathReport` and `forsake` machinery is built
around a client's death and would have to express a host death as a
distinct, more severe event — the difference between "this exit failed" and
"nothing can be acquired".

The exposure is not nominal. Four clients in one address space share fate,
so a panic anywhere in the nym-sdk graph takes all of them, and
`#![forbid(unsafe_code)]` binds our crates and not that dependency tree.
Desktop is giving up something real, and it is giving it up to gain a
property mobile cannot decline: a phone has no subprocess to isolate into,
so mobile already runs exactly this risk and has since it shipped.

Three things blunt it. The host is a supervised child that the wallet
already knows how to restart, so a host death becomes a recoverable event
rather than a session-ending one, on the same footing as the failover that
exists today. Conduit death and host death become distinct typed events, so
a consumer can narrate the difference instead of inferring it. And the
fail-closed route resolver already governs the outcome: losing every
conduit refuses every mixnet-only surface, and never falls back to
clearnet, so the failure mode of a shared host is unavailability rather
than exposure.

The control channel is the largest piece of engineering, and it is not new
design. Today's one-way stdout announcement works only because one process
serves one conduit, so no line has to say which conduit it describes. A
multi-conduit host needs request and response with correlation, lifetimes,
and typed errors — which is the contract `ProxyHosting` already specifies,
carried over a pipe instead of a callback. The work is transporting an
agreed contract, not inventing one.

`wait_for_parent_exit` needs rework. The child's stdin-EOF watchdog assumes
the child dies with the work it was spawned for; a host outliving many
conduits must instead die with the session, and an orphaned host holding
mixnet clients open is a worse outcome than an orphaned single proxy.

What is gained beyond ADR 0047: discovery is paid once per epoch rather
than once per birth against a boot that currently spawns four; a rotation
costs a new client inside a live process rather than a fork, an exec, and a
fresh tokio runtime every five to ten minutes; the health index accumulates
across a session on both platforms for the first time; and there is one
acquisition path to test rather than two that diverge exactly where they
are hardest to exercise.

## Open

Whether a restarted host re-proves its exits or trusts the epoch's standing
observations. ADR 0044's proof-at-birth rule and ADR 0043's Sentinel decide
the mechanism; what a restart inherits is unruled.

Whether the wallet observes host health at all, or only conduit health. The
fail-closed resolver needs no more than "my conduit no longer serves", but
a user interface may want to distinguish a lost host from a lost exit.
