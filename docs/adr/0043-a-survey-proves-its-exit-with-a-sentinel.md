# A survey proves its exit with a Sentinel, and restarts when the proof fails

Status: draft — ratified in session 2026-08-13, pending review;
superseded in part by ADR 0044 the same day: the Sentinel and its budget
survive, but the proof moved from a displaced lane inside every wave to
the client's birth, and the wave returned to full indexer width

## Context

A transport announced itself ready when its Exit Node was bound and its
local SOCKS5 listener was up. Neither fact involves the exit. The nym
SDK's `connect_to_mixnet_via_socks5` establishes our own client and its
gateway connection; it never contacts the exit, so it succeeds against
an exit that carries nothing. The proxy announces the exit identity and
the listening address immediately afterward, and the wallet reads that
pair as readiness.

The clutch race inherits the same blindness. `connect_across_exit_nodes`
races the drawn exits and crowns whichever arm builds its local client
first, which is uncorrelated with whether that exit forwards traffic —
a dead exit wins as readily as a live one, and may win more often.

Measurement over 2026-08-12 and 2026-08-13 made the cost concrete. Two
harness runs of twelve Server-Selection Sweeps each collapsed three
times, and the outcomes were bimodal with nothing between them: a
verdict in 4.8 to 8.7 seconds, or a collapse at 103.5 to 104.5 seconds.
An instrumented collapse showed all seventeen probes failing at exactly
20001 ms at the tunnel stage, including indexers that had answered in
under three seconds through a different exit moments earlier. Two
collapsed surveys re-probed the same tunnel ten seconds later and failed
again, so the exit was dead rather than slow to start. An independent
check through Cloudflare and Google resolvers found one exit in four
silent to both — the same quarter, measured through destinations that
have nothing to do with Zcash.

The 100-second collapse is arithmetic: seventeen candidates at width
four is five waves, and five waves of a 20-second leg budget is 100
seconds. The width bound and the leg budget were calibrated together for
the healthy case, where they deliver a verdict in about six seconds; in
the failed case they multiply.

A first remedy — ending the survey once its opening wave had timed out
to a leg — cut the collapse to 24 seconds but doubled the collapse rate,
from three in twelve to six in twelve, because four slow candidates now
condemned a survey that would have found a healthy indexer later. Cheap
and wrong.

## Decision

A survey proves the exit it rides, and the proof is real traffic.

**Exit-Proven** is the only validation a transport carries: a round trip
has completed through its bound Exit Node. It replaces Exit-Bound, which
named the unproven state as though it were an assurance. The interval
before proof has no name, no type, and no permitted use beyond making
the attempt that proves it.

A speed-priority survey carries one **Sentinel** in its opening wave: a
realistic request to a highly reliable public address, which is not
Correspondable and is never eligible for the cohort or the verdict. The
Sentinel holds a Survey Lane rather than adding one — the survey width
is a ceiling measured for the one Nym client that the mobile host runs
in-process, and a wider opening wave risks a saturation that would be
indistinguishable from a dead exit, so the detector would fire on its
own load. The Sentinel carries its own budget, shorter than the
indexers', because a reliable address that stays silent indicts the
tunnel rather than itself.

Three outcomes race in that wave. A healthy indexer answers, which both
proves the exit and yields the verdict. The Sentinel replies — with
anything, an error included, since only a live exit can deliver one —
which proves the exit, and later waves carry indexers alone. Or the
Sentinel is silent within its budget, which condemns the exit.

A condemned exit ends the attempt, not the sweep. The exit is abandoned
with its reservation held until a replacement binds, so the pool cannot
offer it again; every survey result is discarded rather than carried
forward; lanes are drawn afresh; and nothing from the failed attempt
charges any indexer's Health or reaches the diary, because a
tunnel-phase failure is the exit's and never the Correspondent's. A
sweep may burn a bounded number of exits this way, then refuses with a
typed error naming how many were proven silent.

Privacy-priority acquisitions carry no Sentinel. They are proven by the
first exchange they were acquired to carry, so nothing extra is ever
sent through an exit that a send will use, and the exit still observes
exactly one destination in its life.

Every judgment above is a total function over the survey's results, with
no I/O: which candidates open which lanes, whether the Sentinel replied
or was silent, and whether a healthy verdict exists. Only the probes
themselves are effectful.

## Considered options

**Prove before surveying.** A dedicated proof round trip at acquisition,
before any caller receives the transport. Rejected: it adds a round trip
to every acquisition, and a dedicated probe is distinguishable from real
traffic, so an adversarial exit can serve probes and drop everything
after — a test an adversary can see is a test an adversary passes.

**Prove with an unroutable sentinel.** Dial a documentation-reserved
address and read the exit's refusal as proof. Rejected for the same
distinguishability, and because a constant unroutable target is a
software fingerprint no other traffic produces.

**Raise the leg budget.** Rejected: the failures are timeouts, so a
longer budget makes a collapse slower, never rarer.

**Narrow or widen the survey width.** Rejected: the width was calibrated
against exactly this failure, and neither direction addresses an exit
that carries nothing.

**End the survey on an all-timeout opening wave.** Implemented and
measured: collapse cost fell from 104 to 24 seconds while the collapse
rate doubled, because slow candidates were read as a dead tunnel.
Superseded by the Sentinel, which distinguishes the two.

## Consequences

A dead exit is detected in seconds rather than at the end of a survey,
and it costs a fresh attempt rather than a refused session. With one
exit in four dead, a sweep that burns six exits before refusing reaches
that bound about once in four thousand attempts, while a typical sweep
burns none.

Measured on 2026-08-13, twenty rounds of each operation against a live
mainnet mixnet. The sweep refused three of twelve surveys before this
decision, each after 104 seconds; it now refuses none of twenty, at a mean
of 9.5 seconds to a verdict, with six rounds running long because they
burned a dead exit and surveyed again. The price run failed nine of twenty
before it carried a Sentinel and five to seven of twenty once it did but
could not redraw; sharing the redraw it now fails none of twenty, at a mean
of 7.9 seconds against about 5 seconds before. Both numbers say the same
thing: the exits die at the same rate as ever, and neither operation loses
a session to one.

The cost is latency on the unlucky round. A price run that draws two dead
exits before a live one takes twenty seconds where it used to fail in ten,
and the sweep's slowest round took 27 seconds. That trade — a slower answer
for an answer at all — is the decision this ADR records.

The Sentinel's address becomes a dependency of every speed-priority
survey. It is reached by a request of the shape that address ordinarily
serves, so neither the exit nor the destination sees anything unusual,
but an operator who blocks it makes every survey restart until the bound
is reached.

The opening wave probes one fewer indexer. First-healthy needs a single
answer, so the cost is a marginally later verdict in the healthy case,
paid to make the failed case fast on every platform including the one
that hosts its mixnet client in-process.

The pattern is cheap on the platform with the least headroom. Proving an
exit costs one connection carrying a few dozen bytes each way, resolved in
about a second, and it is the only proof shape that does: racing more Exit
Nodes costs whole mixnet clients, and widening the wave costs concurrent
connections through the one client a mobile app hosts in its own process,
alongside its UI and under a memory cap. A platform that cannot afford
breadth can still afford one round trip, so the same proof serves desktop's
spawned child and the mobile host without a per-platform budget.

`ExitProven` must be reified — a type whose only route to existence is
evidence of a completed round trip — or this ADR describes an intention
rather than a guarantee. The glossary lost an entry on 2026-08-13 for
exactly that failing.
