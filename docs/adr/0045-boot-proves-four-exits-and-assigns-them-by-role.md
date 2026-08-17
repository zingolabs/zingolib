# Boot proves four exits and assigns them by role

Status: draft — ruled in session 2026-08-17, pending review and
implementation

## Context

ADR 0044 moved proof to a client's birth: a Proven Client answers the
Sentinel before it takes any role. ADR 0043 established what the proof
is — a completed round trip through the bound Exit Node, made with real
traffic. Together they retired the Correspondent Pools and left every
operation acquiring its own client on demand.

Acquiring on demand is what boot does not do well. An online session
births one client, which fails closed, and then runs the
Server-Selection Sweep, which does not: a sweep reaching no live exit
returns `false`, sync never launches, and the prompt opens with no
indexer bound. A boot that succeeds by any user-visible measure
therefore guarantees exactly one proven exit.

Everything else pays for that later. A measured first `current_price`
took 30.7 seconds, of which the quote was under two: the rest was a
fresh client's bootstrap, its exit announcement, and its Sentinel. The
run's clock starts before acquisition, so the figure is a whole birth.

Two mechanisms below the roles make this worse, and both predate
proof-at-birth.

The **Clutch** draws four exit reservations and hands all four to one
proxy, which races them and binds whichever announces first. It does buy
something real: a Nym client's bootstrap to a *specific* exit can be slow
or never complete, and with a quarter to a third of exits carrying
nothing (ADR 0043), racing four means a birth finishes at the fastest
rather than waiting out `EXIT_ANNOUNCEMENT_GRACE` on a dud. What it does
not do is select. `draw_clutch` orders candidates fresh-proven first,
unknown next, failed at exhaustion, then collects into a `HashSet` and
lets announcement order decide — so the preference survives only as
*which four* are drawn. It buys its hedge by discarding three-quarters
of the work it starts.

The **speed-priority redraw** (`run_speed_prioritized`) loops up to six
draws under a 285-second deadline, abandoning an exit whenever a wave
goes wholly unanswered. Its own module header records that proof moved to
birth and the redraw survives only "as the safety net for an exit dying
inside its epoch". That trigger cannot tell a dead exit from a dead
cohort: nine price sources or seventeen indexers all failing reads
identically to one exit failing, so the loop convicts the exit on the
cohort's evidence and spends another birth to reach the same answer.

## Decision

**Boot proves four exits, races their bootstraps, and assigns roles in
the order they confirm.**

1. Acquire the set of exits advertised for the current Nym epoch.
2. Start four Nym clients, each attempting a different exit, each
   proving itself by contacting the Sentinel at `1.1.1.1`.
3. The first exit to prove itself becomes the **IndexerSweep exit**, and
   its client runs the sweep.
4. The second becomes the **PriceFetch exit**, and its client fetches the
   price.
5. The third becomes the **IndexerClient exit**, which carries
   transmissions.
6. The fourth is the **spare exit**, taken up when a proven exit starts
   failing.

The role belongs to the exit, not to the client. A client is the
transient instrument that binds an exit and does one job; the exit keeps
its role and its proof for the epoch, so the next client doing that job
binds the same exit.

Racing at this layer replaces the Clutch's hedge and improves on it:
four bootstraps run in parallel as before, but every winner is kept and
given work instead of three being discarded.

**Only the client on the IndexerClient exit persists.** The other three
clients live for their purpose or for boot, then stop. Their exits stay
`EpochProven` and keep their roles, so the set of four is reused for
every request until the epoch ends or an exit starts failing.

**The price is fetched during boot and printed.** No wallet write, no
session store: the CLI narrates the quote, and zingo-mobile does what it
likes with its own result. A later `current_price` at the prompt binds
the PriceFetcher's proven exit again, so it skips the Sentinel.

**Exits carry a role, not only a verdict.** `NodeHealthIndex` records
health per node and nothing records which node holds which role, so the
PriceFetch, IndexerSweep, IndexerClient, and spare exits need a
role-to-exit binding that outlives every client. Health decides whether
an exit may be used at all; the role binding decides which of the proven
four a given operation reaches for.

**A failed sweep aborts boot.** An online session that cannot select a
sync indexer no longer opens an indexerless prompt, so "the session
opened" and "four exits are proven" become one statement.

**The Clutch retires.** A birth draws one exit and binds it, and the
preference order in `draw_clutch` then decides what a birth uses,
because nothing downstream can overrule it.

**The speed-priority redraw retires.** An operation acquires a proven
client and runs one wave. A wave no target answers is reported as the
cohort's failure; an exit is convicted only by its own silence to the
Sentinel. `SpeedPrioritized` survives as the shared shape for the sweep
and the price run; its `abandon` and `answered` members retire with the
loop they served.

## Considered options

**Leave boot as it is.** Rejected: the first price fetch pays a full
birth, measured at thirty seconds, and the Standing Client's is the only
guaranteed proof.

**Prove a third exit without retiring the Clutch.** Rejected as
insufficient rather than wrong: a proven exit a race need not bind
improves the odds and guarantees nothing.

**Keep the Clutch for its hedge.** Rejected because the hedge moves
rather than disappears. Racing four births at boot hedges the same
bootstrap latency and banks four proven exits instead of one.

**Keep the redraw as a safety net.** Rejected: it catches an exit dying
between its Sentinel and its wave, and pays with six births and a false
verdict against a healthy exit whenever a cohort is down.

**Persist all four clients.** Rejected: ADR 0044 records that the mobile
platform hosts each client in-process and tolerates roughly five
concurrent, which is why the Correspondent Pools were retired. One
persistent client keeps steady-state load where it is today.

## Consequences

Boot spends four Sentinel-proven bootstraps before the prompt, in
parallel, so the wall-clock cost is bounded by the slowest of four
rather than their sum. The price arrives during boot at no extra wait.

An online session with every indexer down now fails to boot where it
previously opened a prompt. That prompt was genuinely useful —
`network`, `changeserver`, and every wallet-local command worked from it
— and losing it is the cost this decision accepts, on the ground that an
online session which cannot reach an indexer cannot do what was asked,
and a prompt hiding that is worse than a refusal naming it.

Every request of a kind reuses one exit for the epoch. A fresh client
per fetch still varies the client identity, but an observer at the
price exit sees an epoch of price fetches as one stream, and the same
holds for the sweep and for transmissions. This is weaker than the
per-fetch fresh exit that `lightclient/mixnet.rs` documents today and is
chosen deliberately for the latency and the guarantee it buys.

Exit discovery becomes epoch-scoped rather than seeded once per session,
so it re-queries at an epoch boundary. `NYM_EPOCH` is presently a
sliding hour with a TODO admitting it approximates the real rotation;
this decision depends on that TODO being closed.

`RESERVATION_CLUTCH_SIZE`, the hedged `acquisition_launch_policy`, and
the `arm_race` machinery behind them lose their wallet-side consumer, as
do `MAX_SPEED_EXIT_DRAWS`, `SPEED_ACQUISITION_DEADLINE`,
`SpeedProgress::ExitAbandoned`, and `SpeedError::{NoLiveExit,
DeadlineExhausted}`. The proxy's own `draw_clutch` at
`zingo-netutils/src/nym_proxy.rs` serves a standalone run and is not
retired here.

The spare's trigger needs a definition of failing. The Sentinel's
silence is unambiguous; a wave no target answers is not, and this
decision has already ruled that case the cohort's.
