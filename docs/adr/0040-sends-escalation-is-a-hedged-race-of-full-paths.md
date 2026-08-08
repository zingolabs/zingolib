# Send's escalation is a hedged race of full paths

Status: draft — ratified in session 2026-08-08, pending review

## Context

ADR 0011 ratified send's escalation as serially gated rounds: one
Correspondent, then two in parallel, then three, each round launching
only after the complete failure of the round before it, capped at six
distinct Correspondents. That schedule sat above a separate
acquisition race that established the tunnel, so a send stacked two
races with different disciplines.

Three later decisions changed the ground under it. The responsiveness
axis was restated as the tradeoff an operation declares, and send
declares PrioritisePrivacy: parsimony governs its widening. The
exit-use partition (ADR 0039) made a send arm's exit Exclusive — one
Correspondent per exit, enforced in the lease type. And the 2026-08-07
session ruled that send's connection establishment follows the Happy
Eyeballs hedged race of RFC 8305, with an interval long enough that a
responsive Correspondent wins before a second arm launches.

The rounds schedule also had a latency defect the hedged shape
repairs: a Correspondent that accepts a submission and stalls silently
holds its whole round open until the arm times out, because rounds
widen only on complete failure, never on silence.

## Decision

One hedged race replaces the serially gated rounds as send's
escalation. Its arms are full paths: each arm pairs a freshly drawn
Correspondent (Correspondent Selection, repetition-free) with its own
Exclusive Exit Node, so acquisition and escalation collapse into a
single race and no exit ever observes a second Correspondent. The
race launches one arm first; a further arm launches only after a
silence interval or an arm's failure; the first confirmed delivery
wins and every other arm is cancelled. The cap of
MAX_TRANSMISSION_CORRESPONDENTS distinct Correspondents stands, as
does ADR 0011's adversary model: the censoring Correspondent that
accepts, then suppresses or misreports.

The hedge interval is chosen to protect the happy path: long enough
that a responsive Correspondent's confirmed delivery beats the first
hedge, so the common send contacts exactly one Correspondent through
exactly one exit. Its value is a named constant whose calibration is
decided separately; this record fixes the discipline, not the number.

## Consequences

The happy path's privacy is unchanged and now timer-protected rather
than schedule-protected: one Correspondent, one exit. Widening spends
exposure only on evidence — silence or failure — which is the
PrioritisePrivacy posture applied to the escalation itself.

A silently stalling Correspondent costs one hedge interval instead of
a full per-arm timeout, so the censored-send worst case improves
without widening faster than the rounds did on the failure path.

ADR 0011's escalation schedule is superseded; its cap, its adversary
model, and its delivery-confirmation requirement stand. The
EscalatingRounds launch policy loses its send-path consumer, and its
retirement or retention becomes an implementation question for the
race planner once the full-path race is built on the reservation
machinery of ADR 0038.
