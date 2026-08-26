# An Exit Node Reservation is unique to its holder

Status: draft — ratified in session 2026-08-07, pending review

## Context

Concurrent transports must not share an Exit Node: a shared exit lets
one observer correlate operations the design promises to keep disjoint.
The code enforces disjointness today by exclusion sets — each acquiring
transport receives a list of the exits already in use, passed across the
process seam as repeated `--exclude-exit` flags — and by a bounded crawl
that contacts up to `MAX_EXIT_NODE_ATTEMPTS` (ten) exits from the
shuffled directory answer.

Exclusion is a negative invariant maintained by every caller separately,
and it fails silently: the 2026-08-07 review of the pool discovery
prototype (issue #2648) found three call sites passing empty exclusion
sets, each a correlation hazard no compiler or test had caught. The
crawl adds a second defect: its depth (ten) is an independent parameter
beside the race's width (`RESERVATION_CLUTCH_SIZE`, three), so the
design carries two arbitrary quantities where it ratified one.

## Decision

Exclusive use of an Exit Node is enforced by ownership, not by
exclusion. The session holds one **Exit Pool** (the glossary's terms
govern; see `zingolib/CONTEXT.md`): the population of eligible Exit
Nodes, discovered once per session, and the sole issuer of **Exit Node
Reservations**. The pool holds exactly one reservation per node, a draw
transfers it, and no two holders ever hold a reservation for the same
node. Disjointness among live transports follows from uniqueness alone,
so no transport need learn what any other bound.

A transport acquires by drawing a **Clutch** — a uniform random sample
of exactly `RESERVATION_CLUTCH_SIZE` reservations — and racing its
connection over the clutch's nodes. The reservation whose node it binds
becomes its **Exclusive Lease**; every other reservation recycles the
moment the lease exists. A transport that exhausts its clutch dies, and
its parent recycles the spent clutch and draws a fresh one for the
respawn. The clutch is therefore both the acquisition's width and its
whole attempt budget: the design's only three-wide quantity.

Amendment 2026-08-10: the ratified **Session Retirement** clause —
per-node failure evidence withholding a node whose failure rate stands
more than one standard deviation above the pool's mean — is excised
before implementation review completed. The statistic retired a node on
its first charged failure over a mostly-zero population (issue #2661,
finding 5), and no reinstatement existed. The pool now assumes every
discovered Exit Node is somewhat viable: population hygiene belongs to
the upstream directory, which the session re-fetches at seed time, and
any future quality filter adopts the nym-api's own performance
annotations rather than an in-wallet statistic
(`.agent-plans/exit-retirement-excision.md`).

## Consequences

The exclusion machinery retires with the crawl: `--exclude-exit`, the
`excluded` parameters, `NymProxyError::AllExitsExcluded`, and
`MAX_EXIT_NODE_ATTEMPTS` all die when the reservation model lands. An
acquisition can no longer be launched with a forgotten exclusion set,
because it cannot be launched without a clutch.

Respawn moves to the parent. A transport no longer redraws internally
after exhausting its attempts; it dies, and the supervisor that owns the
pool decides whether to respawn with a fresh clutch. The Destination
Pools this enables race their acquisitions as `PrioritisePrivacy`,
activating the second responsiveness class.

The model is ratified but not yet implemented; the glossary entries and
this record govern the implementation (plan items 1 and 3 of
`responsiveness-plan.md`). ADR 0035's arm/pull distinction becomes
load-bearing here: a respawned transport may pull the same arm in a
later clutch, so the race accounts in pulls, never in arms.
