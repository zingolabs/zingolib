# The acquisition race speaks the literature's vocabulary

Status: draft — ratified in session 2026-08-07, pending review

## Context

The connection-acquisition machinery grew a working vocabulary — race,
arm, hedge — drawn from three bodies of literature but never anchored
to them. The 2026-08-07 review of `responsiveness.rs` found one drift
that anchoring would have prevented: the code names an in-flight
connect attempt an "arm," while in the coining literature the arm is
the contender and one trial of it is a pull. The conflation is harmless
today only because the race tries each contender at most once, so arms
and pulls are accidentally one-to-one. The Exit Node Reservation model
breaks that coincidence the moment a reservation may be attempted twice
within one clutch.

## Decision

The acquisition machinery uses the conventional terms exactly as the
source literature defines them, and each term is anchored to the work
that coined it.

The canonical term is the **Acquisition Race**: winner-take-all
concurrent redundancy in pursuit of one connection, where several
attempts race, the first success is bound, and every loser is
cancelled. The sense is the connection racing of Happy Eyeballs, coined
in RFC 6555 (D. Wing and A. Yourtchenko, "Happy Eyeballs: Success with
Dual-Stack Hosts," 2012) and elaborated as an explicit racing framework
in RFC 8305 (D. Schinazi and T. Pauly, "Happy Eyeballs Version 2:
Better Connectivity Using Concurrency," 2017). Code spells the term
`acq_race`: types carry `AcqRace`, and functions and variables carry
`acq_race`, so an identifier names the Acquisition Race explicitly
rather than a bare race.

An **arm** is a contender available to be tried — in an acquisition,
one exit reservation of the clutch. A **pull** is one trial of an arm —
one connect attempt. The vocabulary is the multi-armed bandit's, from
the sequential-design problem posed by H. Robbins ("Some Aspects of the
Sequential Design of Experiments," Bulletin of the American
Mathematical Society 58(5):527–535, 1952), whose slot-machine metaphor
(the "one-armed bandit") supplies both words: the machine is the arm,
and a play of it is a pull.

A **hedged** launch defers redundancy: one pull first, another only
after a quiet interval or a failure. The term is the hedged request of
J. Dean and L. A. Barroso ("The Tail at Scale," Communications of the
ACM 56(2):74–80, 2013). Hedging names a launch policy within a race,
never the race itself.

## Consequences

Code that conflates arm with pull is misnamed and will be corrected as
its own change: the race's unit of accounting is the pull, and the
clutch counts arms. The distinction becomes load-bearing when pulls per
arm exceed one.

Reviewers can test a proposed name against a citation instead of a
preference: a term either matches its coining literature or it is not
used.
