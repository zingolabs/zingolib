# A client proves its exit before its first role, and later clients trust fresh proof

Status: draft — ratified in session 2026-08-13, pending review; the
two-layer failover ruling was added in session 2026-08-14

## Context

Landing the Destination Pools slowed the scan, and the whole chain is
measured. A bisect over 557 commits, judging each commit by sync time
alone through a session-printed mark, confirmed its boundary five
samples a side with no overlap: the parent of the Pools landing scanned
a 5,000-block window in 66.6–72.2 seconds, and the next measurable
commit scanned it in 81.2–90.0. The landing itself (`0df2492fa`),
unblocked by a benchmark-local override, measured 80.3–82.3. A knockout
run at the same commit with the go-online `ensure_filled` call disabled
returned to 70.8–73.3 seconds. The mechanism is not on the scan path:
go-online launched three background pool refills, each racing a Clutch
of mixnet clients, and roughly a dozen concurrent client bootstraps
contended with the scan for its whole opening minute.

The pools existed to hide acquisition latency behind readiness, and
their members had to be many because acquisition is unreliable:
measurement over 2026-08-12/13 put a quarter to a third of Nym Exit
Nodes at carrying nothing (ADR 0043), so every acquisition hedged with
a four-exit Clutch. The session therefore paid for dead exits twice —
in Clutch width at every acquisition, and in pool complements sized to
survive consumption. Meanwhile the mobile platform bounds the whole
budget: it hosts each Nym client in-process and supports roughly five
concurrent clients before the load is itself a defect.

Two facts from the vendor stack shape any remedy. A `Socks5MixnetClient`
pins its one egress provider at construction, so "trying another exit"
means building another client, and the expensive step is the client's
own gateway registration, not the exit. And Nym rotates its topology
each epoch, so any observation about an exit expires when the epoch
does.

## Decision

Proof becomes the first act of a client's life, and accumulated proof
replaces standing readiness.

The session keeps one `NodeHealthIndex`: a map from Exit Node to its
single most recent `Observation`, a verdict with a timestamp. A verdict
is `Proven` — a completed round trip through the exit: it answered the
Sentinel query to 1.1.1.1, or it carried a task — or `Failed` — it
refused, timed out, or stayed silent past budget. `Proven` is fresh for
one `NYM_EPOCH` (Nym's topology-rotation interval, one hour) and then
lapses; `Failed` stands for the session, because retrying known-dead
exits spends the census on nothing. Only the Exit Pool's recycle path
writes the index, so an observation can come only from a Reservation
the pool itself issued.

At boot the session provisions three clients immediately, each born
over its own Reserved exit, each proving its exit against 1.1.1.1
before taking any role. Roles bind in order of proof, not birth: the
first Proven Client becomes the Index Sweeper, the second the Price
Fetcher, the third stands by for application operations. A birth whose
probe refuses remembers `Failed`, dies, and is succeeded by a birth
over the next candidate. A client turns off when its task completes;
its Reservation recycles and its exit's `Proven` entry persists.

The three exits those births prove are the session's whole egress: a
working set of three that the session never widens. After the boot
round the session runs one standing Proven Client — the third birth —
and every operation except the price fetch multiplexes over it: one
client carries many concurrent streams, so concurrency needs no
further client and no queue. The price fetch keeps its own client: a
short-lived Proven Client raised per run over a Fresh Exit and turned
off behind the quote, so the priced traffic never shares an egress
with the wallet-correlated streams on the standing client. The
remaining working-set exits stand by as the session's Fresh Exits:
proven, unreserved, held only in the index. A successor client draws
a Fresh Exit and goes straight to work — no probe, because within its
epoch proof is trusted; a proving birth covers the draw when no Fresh
Exit stands.

Whenever a working-set exit fails — a birth probe refused at boot, or
the standing client's exit failing under it at any later moment — the
failure writes `Failed` through the recycle path; the session fails
over by raising a new client on a Fresh Exit, and a bootstrap-style
proving birth restores the working set to three: draw an unknown
exit, avoiding `Failed`; bootstrap; prove; join in the failed exit's
place. The set restores itself by the same act that convicts its
members. Use also renews proof: a completed task is a round trip, so
the live exit's `Proven` entry refreshes through the recycle path and
only the standby exits' proof decays with the epoch — an expired
standby is simply an unknown again, re-proven by the next proving
birth that draws it.

The failover lives below the Mixnet Mode, by the two-layer ruling of
2026-08-14. An exit failing under a live client and the transport
itself dying are different events, and only the second is `Died`: the
exit failure leaves the proxy process and its SOCKS5 listener
standing, so it writes `Failed` and triggers the failover, while ADR
0011's `Died` keeps naming exactly one thing — the unconsented loss
of the transport — and stays latched until an explicit re-enable.
While the failover's replacement client bootstraps the session cannot
truthfully claim readiness, so the mode shows `Bootstrapping`, whose
surfaces already refuse, and returns to `Ready` only when a Proven
Client stands again. A failover that exhausts — `NoProvenExit` —
lands `Died`. ADR 0024's rule that the driver owns the recovery
predicate is preserved rather than amended: the driver never leaves
`Died` on its own, because the failover is not a recovery but the
continuation of the enable the user already consented to, carried
over a different exit.

Every draw passes through the Exit Pool's Reservation gate, which the
index never bypasses: the index orders sampling — fresh-`Proven`
first, then unknown, `Failed` only at exhaustion — and the Reservation
alone confers use. A draw that cannot be satisfied — no working-set
exit stands and proving births are failing — waits up to a bound
derived from the client bootstrap ceiling plus a small multiple of the
Sentinel budget, then returns the typed refusal `NoProvenExit` naming
what was probed and for how long; the operation maps that refusal to
its caller and the existing refusal chains carry it.

The policy is platform-typed along ADR 0041's seam only at its edges.
Both platforms converge on the same steady state — one standing
client for everything but the price fetch — because the mobile
platform supports neither several concurrent clients nor frequent
setup and teardown, and the desktop no longer wants them. The
spawning platform runs the full shape: the boot trio, the per-run
price client, local respawn on failover. The attached session keeps
its one host-provisioned client for its whole life, runs the price
fetch over that same endpoint as its platform constraint, performs
its birth probe through the host's endpoint at attach, and surfaces a
later exit failure to the host as a request for a fresh endpoint.

The member-keeping pools retire. `ensure_filled`, the refill tasks, the
Indexer and Price complements, and the go-online fill all leave; the
Exit Pool remains as the session's one exit authority — Reservations,
the acquisition-load ledger, and the `NodeHealthIndex`. The Sentinel
lane leaves the survey and price waves, since a wave now runs only over
a tunnel whose exit was proven at the tunnel's birth; the operation
keeps one redraw as the safety net for an exit dying inside its epoch,
and a task failure over a trusted exit writes `Failed` through the
recycle path, so no exit betrays two tasks.

## Considered options

**Keep the pools and schedule refills into quiet time.** The first
design ratified in this session: an acquisition-load ledger, refills
deferred while a sync runs, serial refill admission. Rejected once the
census economics were on the table: it keeps twelve-client spikes and
standing cover traffic, adds a scheduler, and the pools' members were
never consumed by the paths that mattered.

**A dedicated Surveyor walking the census.** One or two walkers
classifying every exit by Sentinel probe, operations waiting on the
resulting stock. Under the stock client's one-provider constraint each
probe pays a full gateway registration, so an exhaustive pass costs an
epoch or more and even a five-exit stock costs a minute of dedicated
walking; a raw-client probe engine (one bootstrap, ~3.5 s per probe)
repairs the economics but is real engineering against the nym crates.
Folded in rather than rejected: the boot three are the walk, sized to
exactly what boot needs, and the probe engine remains the follow-up
that would make proving births cheap.

**Trust readiness, as before ADR 0043.** Rejected on the measurements
in that ADR's context: readiness proves nothing about the exit.

## Consequences

Ambient load is bounded by construction, not scheduling: three clients
at boot, roughly one thereafter, against the dozen the pools spawned —
and the mobile ceiling of about five concurrent clients is respected
with room for the operations themselves. The scan runs clearnet, the
Sweeper and Fetcher turn off behind their tasks, and the knockout's
8–10 seconds of scan-time contention has no remaining source.

The privacy trade is explicit and narrow. All non-price operations
correlate at the standing client's one exit — the prior architecture
paid a dozen clients to scatter them, and this decision judges that
price wrong for the wallet's traffic shape and client-cost reality.
The one separation retained is the one with the sharpest teeth: the
price fetch rides its own per-run client, so priced traffic never
shares an egress with wallet-correlated streams. Where correlation at
the standing exit ever measures as a real exposure, the remedy is
raising more per-class clients within the working set, not widening
the set.

Boot latency becomes time-to-first-proof: one client bootstrap plus a
Sentinel budget when the first drawn exit is live, one succession more
when it is not. The client bootstrap figure is deliberately
measured-later; the sweep inherits the fastest-proving exit of three by
construction. Steady-state task latency drops to a bare bootstrap
whenever fresh proof exists.

ADR 0043 is superseded in part: the Sentinel and its budget survive,
but the proof moves from a displaced lane inside every wave to the
client's birth, and the wave returns to full indexer width. ADR 0038/
0039's Reservation and lease semantics are unchanged and now mediate
every draw including probes. ADR 0011's five-state mode and ADR
0024's driver-owned recovery predicate are likewise unchanged: the
failover passes through `Bootstrapping`, and `Died` — now reachable
from failover exhaustion as well as proxy death — is still left only
by an explicit re-enable. The `NodeHealthIndex` is in-memory and
epoch-scoped by design; issue #2703's persistent Exit History becomes a
second consumer of the same recycle-path evidence when its `Memorable`
extraction lands. Issue #2704 (an edge-carrying sync-lifecycle surface)
stops mattering to this design — no scheduler consults sync state —
but stands on its own merits.

The stock `Socks5MixnetClient` remains the only client shape; nothing
below the SOCKS5 seam changes. The raw-client probe engine — one
bootstrapped client probing many providers at ~3.5 s each — is the
named follow-up if proving births ever dominate boot, and it would
also let the Clutch narrow at every demand site.
